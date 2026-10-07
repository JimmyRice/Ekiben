//! HTTP handlers: each turns a request into one [`service`](super::service) call and its
//! result into a response. Rules live in the service, not here.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use kippu_domain::reservation::Reservation;
use kippu_domain::{AttestorId, ReservationId};
use kippu_store::OutboxRecord;

use super::dto::{
    AttestationRequest, AttestorKeyRequest, AttestorView, CreateAttestorRequest, FeedEvent,
    FeedQuery, ManualPaymentRequest, RevokedRequest, SettlementView,
};
use super::service::{self, AttestorReport, PaymentResult};
use super::signature::Attested;
use crate::app::AppState;
use crate::auth::Principal;
use crate::error::{ApiResult, Problem};
use crate::http::idempotency::IdempotentByDesign;
use crate::http::{Json, Listing};

const TAG: &str = "payments";

/// A settlement response, marked as safe to repeat without the idempotency middleware.
fn settlement(result: PaymentResult) -> Response {
    let mut response = Json(SettlementView::from(result)).into_response();
    response.extensions_mut().insert(IdempotentByDesign);
    response
}

fn feed(records: Vec<OutboxRecord>) -> Json<Vec<FeedEvent>> {
    Json(records.into_iter().map(FeedEvent::from).collect())
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
    let attestor = service::create_attestor(&state, &principal, request.into()).await?;
    Ok((StatusCode::CREATED, Json(attestor.into())))
}

/// Every attestor, including the built-in `free` and `manual` ones.
#[utoipa::path(
    get, path = "/v1/admin/attestors", tag = TAG,
    security(("bearer" = [])),
    responses((status = 200, body = Listing<AttestorView>), (status = 403, body = Problem))
)]
pub(crate) async fn list_attestors(
    State(state): State<AppState>,
    principal: Principal,
) -> ApiResult<Json<Listing<AttestorView>>> {
    let attestors = service::attestors(&state, &principal).await?;
    Ok(Json(Listing::all(
        attestors.into_iter().map(AttestorView::from).collect(),
    )))
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
    service::set_attestor_revoked(&state, &principal, attestor_id, request.revoked).await?;
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
    service::add_attestor_key(&state, &principal, attestor_id, request.into()).await?;
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
    service::set_attestor_key_revoked(&state, &principal, attestor_id, &key_id, request.revoked)
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
    Ok(Json(
        service::reservation_for_attestor(&state, &attested.attestor, reservation_id).await?,
    ))
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
    let report = AttestorReport::try_from(request)?;
    let result = service::record_attestation(&state, &attested.attestor, report).await?;
    Ok(settlement(result))
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
    let result =
        service::manual_payment(&state, &principal, reservation_id, request.into()).await?;
    Ok(settlement(result))
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
    let (after, limit) = query.page();
    Ok(feed(
        service::attestor_feed(&state, &attested.attestor, after, limit).await?,
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
    let (after, limit) = query.page();
    Ok(feed(
        service::full_feed(&state, &principal, after, limit).await?,
    ))
}
