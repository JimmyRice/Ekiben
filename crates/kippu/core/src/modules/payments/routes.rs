use crate::http::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use kippu_domain::outbox::IntegrationEvent;
use kippu_domain::payment::{Attestor, AttestorKey};
use kippu_domain::reservation::Reservation;
use kippu_domain::validation::non_empty;
use kippu_domain::{AttestorId, ReservationId, ValidationError};

use super::dto::{
    AttestationRequest, AttestorKeyRequest, AttestorKeyView, AttestorView, CreateAttestorRequest,
    FeedEvent, FeedQuery, ManualPaymentRequest, PaymentOutcome, RevokedRequest, SettlementView,
};
use super::permissions::{ATTESTORS_MANAGE, FEED_READ, PAYMENTS_MANUAL};
use super::service::{IncomingPayment, confirm_refund, settle};
use super::signature::Attested;
use crate::app::AppState;
use crate::auth::{Principal, Scope};
use crate::error::{ApiError, ApiResult, Problem};
use crate::http::idempotency::IdempotentByDesign;
use crate::keys::{encode_key, parse_verifying_key};

const TAG: &str = "payments";

async fn attestor_view(state: &AppState, attestor: Attestor) -> ApiResult<AttestorView> {
    let keys = state.store().attestor_keys(attestor.id).await?;
    Ok(AttestorView {
        attestor,
        keys: keys
            .into_iter()
            .map(|key| AttestorKeyView {
                key_id: key.key_id,
                public_key: encode_key(&key.public_key),
                revoked: key.revoked,
            })
            .collect(),
    })
}

fn attestor_key(
    state: &AppState,
    attestor: AttestorId,
    request: &AttestorKeyRequest,
) -> ApiResult<AttestorKey> {
    non_empty("key_id", &request.key_id, 64)?;
    let public_key = parse_verifying_key("public_key", &request.public_key)
        .map_err(|_| ValidationError::new("public_key", "must be an Ed25519 public key"))?;
    Ok(AttestorKey {
        attestor_id: attestor,
        key_id: request.key_id.clone(),
        public_key: public_key.to_bytes(),
        revoked: false,
        created_at: state.now(),
    })
}

/// Register an attestor: a service trusted to report payments.
#[utoipa::path(
    post, path = "/v1/admin/attestors", tag = TAG,
    security(("bearer" = [])),
    request_body = CreateAttestorRequest,
    responses((status = 201, body = AttestorView), (status = 403, body = Problem))
)]
pub(crate) async fn create_attestor(
    State(state): State<AppState>,
    principal: Principal,
    Json(request): Json<CreateAttestorRequest>,
) -> ApiResult<(StatusCode, Json<AttestorView>)> {
    state.authorize(&principal, ATTESTORS_MANAGE, Scope::Global)?;
    non_empty("name", &request.name, 200)?;
    let attestor = Attestor {
        id: AttestorId::generate(),
        name: request.name,
        environment: request.environment,
        revoked: false,
        created_at: state.now(),
    };
    let keys = request
        .keys
        .iter()
        .map(|key| attestor_key(&state, attestor.id, key))
        .collect::<ApiResult<Vec<_>>>()?;
    state.store().insert_attestor(&attestor).await?;
    for key in &keys {
        state.store().insert_attestor_key(key).await?;
    }
    state
        .audit(&principal, "attestor.create", attestor.id)
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(attestor_view(&state, attestor).await?),
    ))
}

/// Every attestor, including the built-in `free` and `manual` ones.
#[utoipa::path(
    get, path = "/v1/admin/attestors", tag = TAG,
    security(("bearer" = [])),
    responses((status = 200, body = Vec<AttestorView>), (status = 403, body = Problem))
)]
pub(crate) async fn list_attestors(
    State(state): State<AppState>,
    principal: Principal,
) -> ApiResult<Json<Vec<AttestorView>>> {
    state.authorize(&principal, ATTESTORS_MANAGE, Scope::Global)?;
    let mut views = Vec::new();
    for attestor in state.store().list_attestors().await? {
        views.push(attestor_view(&state, attestor).await?);
    }
    Ok(Json(views))
}

/// Revoke or restore an attestor. Takes effect immediately on every instance.
#[utoipa::path(
    put, path = "/v1/admin/attestors/{attestor_id}/revoked", tag = TAG,
    security(("bearer" = [])),
    params(("attestor_id" = AttestorId, Path)),
    request_body = RevokedRequest,
    responses((status = 204), (status = 404, body = Problem))
)]
pub(crate) async fn set_attestor_revoked(
    State(state): State<AppState>,
    principal: Principal,
    Path(attestor_id): Path<AttestorId>,
    Json(request): Json<RevokedRequest>,
) -> ApiResult<StatusCode> {
    state.authorize(&principal, ATTESTORS_MANAGE, Scope::Global)?;
    if !state
        .store()
        .set_attestor_revoked(attestor_id, request.revoked)
        .await?
    {
        return Err(ApiError::not_found("attestor"));
    }
    let action = if request.revoked {
        "attestor.revoke"
    } else {
        "attestor.restore"
    };
    state.audit(&principal, action, attestor_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Register another signing key, e.g. to rotate keys.
#[utoipa::path(
    post, path = "/v1/admin/attestors/{attestor_id}/keys", tag = TAG,
    security(("bearer" = [])),
    params(("attestor_id" = AttestorId, Path)),
    request_body = AttestorKeyRequest,
    responses((status = 204), (status = 404, body = Problem), (status = 409, body = Problem))
)]
pub(crate) async fn add_attestor_key(
    State(state): State<AppState>,
    principal: Principal,
    Path(attestor_id): Path<AttestorId>,
    Json(request): Json<AttestorKeyRequest>,
) -> ApiResult<StatusCode> {
    state.authorize(&principal, ATTESTORS_MANAGE, Scope::Global)?;
    let attestor = state
        .store()
        .attestor(attestor_id)
        .await?
        .filter(|attestor| !attestor.id.is_builtin())
        .ok_or_else(|| ApiError::not_found("attestor"))?;
    state
        .store()
        .insert_attestor_key(&attestor_key(&state, attestor.id, &request)?)
        .await?;
    state
        .audit(
            &principal,
            "attestor.key.add",
            format!("{attestor_id}/{}", request.key_id),
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Revoke or restore one signing key.
#[utoipa::path(
    put, path = "/v1/admin/attestors/{attestor_id}/keys/{key_id}/revoked", tag = TAG,
    security(("bearer" = [])),
    params(("attestor_id" = AttestorId, Path), ("key_id" = String, Path)),
    request_body = RevokedRequest,
    responses((status = 204), (status = 404, body = Problem))
)]
pub(crate) async fn set_attestor_key_revoked(
    State(state): State<AppState>,
    principal: Principal,
    Path((attestor_id, key_id)): Path<(AttestorId, String)>,
    Json(request): Json<RevokedRequest>,
) -> ApiResult<StatusCode> {
    state.authorize(&principal, ATTESTORS_MANAGE, Scope::Global)?;
    if !state
        .store()
        .set_attestor_key_revoked(attestor_id, &key_id, request.revoked)
        .await?
    {
        return Err(ApiError::not_found("attestor key"));
    }
    state
        .audit(
            &principal,
            "attestor.key.revoke",
            format!("{attestor_id}/{key_id}"),
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// For attestors: the authoritative amount and state of a reservation, before charging.
#[utoipa::path(
    get, path = "/v1/attestor/reservations/{reservation_id}", tag = TAG,
    params(("reservation_id" = ReservationId, Path), ("Kippu-Signature" = String, Header)),
    responses((status = 200, body = Reservation), (status = 401, body = Problem), (status = 404, body = Problem))
)]
pub(crate) async fn attestor_reservation(
    State(state): State<AppState>,
    Path(reservation_id): Path<ReservationId>,
    attested: Attested,
) -> ApiResult<Json<Reservation>> {
    let reservation = state
        .store()
        .reservation(reservation_id)
        .await?
        .ok_or_else(|| ApiError::not_found("reservation"))?;
    let sale = state
        .store()
        .sale(reservation.sale_id)
        .await?
        .ok_or_else(|| ApiError::not_found("sale"))?;
    if !sale.accepts(attested.attestor.id) {
        return Err(ApiError::not_found("reservation"));
    }
    Ok(Json(reservation))
}

/// For attestors: report a payment, or confirm a refund Kippu asked for.
///
/// Safe to retry: an attestation is recorded once per `attestation_id`, and every delivery
/// gets the same answer. A response with status 200 means "recorded — stop retrying", even
/// when the disposition is `refund_required`.
#[utoipa::path(
    post, path = "/v1/payment-attestations", tag = TAG,
    params(("Kippu-Signature" = String, Header)),
    request_body = AttestationRequest,
    responses(
        (status = 200, body = SettlementView),
        (status = 401, body = Problem),
        (status = 403, description = "Attestor not accepted, or sandbox attestor on a live sale", body = Problem),
        (status = 422, description = "Amount does not match the reservation", body = Problem),
    )
)]
pub(crate) async fn record_attestation(
    State(state): State<AppState>,
    attested: Attested,
) -> ApiResult<Response> {
    let request: AttestationRequest = attested.json()?;
    let settlement = match request.outcome {
        PaymentOutcome::Paid => {
            let payment = IncomingPayment {
                attestation_id: request.attestation_id,
                reservation_id: request.reservation_id.ok_or(ValidationError::new(
                    "reservation_id",
                    "is required for a payment",
                ))?,
                amount: request
                    .amount
                    .ok_or(ValidationError::new("amount", "is required for a payment"))?,
                occurred_at: request.occurred_at.unwrap_or_else(|| state.now()),
            };
            settle(&state, &attested.attestor, payment).await?
        }
        PaymentOutcome::Refunded => {
            confirm_refund(&state, attested.attestor.id, &request.attestation_id).await?
        }
    };
    let mut response = Json(settlement).into_response();
    response.extensions_mut().insert(IdempotentByDesign);
    Ok(response)
}

/// Record a payment taken in person (e.g. cash at the venue).
#[utoipa::path(
    post, path = "/v1/reservations/{reservation_id}/manual-payment", tag = TAG,
    security(("bearer" = [])),
    params(("reservation_id" = ReservationId, Path)),
    request_body = ManualPaymentRequest,
    responses((status = 200, body = SettlementView), (status = 403, body = Problem), (status = 422, body = Problem))
)]
pub(crate) async fn manual_payment(
    State(state): State<AppState>,
    principal: Principal,
    Path(reservation_id): Path<ReservationId>,
    Json(request): Json<ManualPaymentRequest>,
) -> ApiResult<Response> {
    let reservation = state
        .store()
        .reservation(reservation_id)
        .await?
        .ok_or_else(|| ApiError::not_found("reservation"))?;
    let event = state
        .store()
        .event(reservation.event_id)
        .await?
        .ok_or_else(|| ApiError::not_found("event"))?;
    state.authorize(
        &principal,
        PAYMENTS_MANUAL,
        Scope::Organization(event.organization_id),
    )?;
    let manual = state
        .store()
        .attestor(AttestorId::MANUAL)
        .await?
        .ok_or_else(|| ApiError::internal("the built-in manual attestor is missing"))?;
    let payment = IncomingPayment {
        attestation_id: request.attestation_id,
        reservation_id,
        amount: request.amount,
        occurred_at: state.now(),
    };
    let settlement = settle(&state, &manual, payment).await?;
    state
        .audit(&principal, "payment.manual", reservation_id)
        .await?;
    let mut response = Json(settlement).into_response();
    response.extensions_mut().insert(IdempotentByDesign);
    Ok(response)
}

fn page(query: &FeedQuery) -> (i64, u32) {
    (
        query.after.unwrap_or(0),
        query.limit.unwrap_or(100).clamp(1, 500),
    )
}

/// For attestors: integration events addressed to you (`payment.requested`, `refund.required`).
/// Poll it with the last sequence number you processed.
#[utoipa::path(
    get, path = "/v1/attestor/feed", tag = TAG,
    params(FeedQuery, ("Kippu-Signature" = String, Header)),
    responses((status = 200, body = Vec<FeedEvent>), (status = 401, body = Problem))
)]
pub(crate) async fn attestor_feed(
    State(state): State<AppState>,
    Query(query): Query<FeedQuery>,
    attested: Attested,
) -> ApiResult<Json<Vec<FeedEvent>>> {
    let (after, limit) = page(&query);
    let me = attested.attestor.id;
    let records = state.store().outbox_after(after, limit).await?;
    Ok(Json(
        records
            .into_iter()
            .filter(|record| match &record.event {
                IntegrationEvent::PaymentRequested { attestor_id, .. }
                | IntegrationEvent::RefundRequired { attestor_id, .. } => *attestor_id == me,
                _ => false,
            })
            .map(|record| FeedEvent {
                sequence: record.sequence,
                created_at: record.created_at,
                event: record.event,
            })
            .collect(),
    ))
}

/// Every integration event, for operators and integrations.
#[utoipa::path(
    get, path = "/v1/admin/feed", tag = TAG,
    security(("bearer" = [])),
    params(FeedQuery),
    responses((status = 200, body = Vec<FeedEvent>), (status = 403, body = Problem))
)]
pub(crate) async fn admin_feed(
    State(state): State<AppState>,
    principal: Principal,
    Query(query): Query<FeedQuery>,
) -> ApiResult<Json<Vec<FeedEvent>>> {
    state.authorize(&principal, FEED_READ, Scope::Global)?;
    let (after, limit) = page(&query);
    let records = state.store().outbox_after(after, limit).await?;
    Ok(Json(
        records
            .into_iter()
            .map(|record| FeedEvent {
                sequence: record.sequence,
                created_at: record.created_at,
                event: record.event,
            })
            .collect(),
    ))
}
