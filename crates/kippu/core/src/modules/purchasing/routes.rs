use crate::http::Json;
use axum::extract::{Path, State};
use axum::http::header::LOCATION;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use kippu_domain::admission::AdmissionPolicy;
use kippu_domain::purchase::{Basket, PurchaseRequest};
use kippu_domain::reservation::Reservation;
use kippu_domain::validation::IdempotencyKey;
use kippu_domain::{PurchaseRequestId, ReservationId, SaleId, ValidationError};

use super::dto::{CheckoutRequest, PurchaseRequestBody};
use super::permissions::{PURCHASES_CREATE, RESERVATIONS_READ};
use super::service::{cancel, checkout, submit};
use crate::app::AppState;
use crate::auth::{Principal, Scope};
use crate::error::{ApiError, ApiResult, Problem};
use crate::http::idempotency::{IDEMPOTENCY_KEY, IdempotentByDesign};
use crate::modules::admission::ADMISSION_PASS;
use crate::modules::catalog::service::visible_sale;

const TAG: &str = "purchasing";

fn header<'h>(headers: &'h HeaderMap, name: &str) -> Option<&'h str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

/// A reservation the caller may see: their own (admins see all). Others look missing.
async fn own_reservation(
    state: &AppState,
    principal: &Principal,
    id: ReservationId,
) -> ApiResult<Reservation> {
    let reservation = state
        .store()
        .reservation(id)
        .await?
        .ok_or_else(|| ApiError::not_found("reservation"))?;
    if state.policy().permits(
        principal,
        RESERVATIONS_READ,
        Scope::Account(reservation.account_id),
    ) {
        Ok(reservation)
    } else {
        Err(ApiError::not_found("reservation"))
    }
}

/// Ask to buy tickets. Returns at once with a queued request to poll; a worker then reserves
/// the tickets or rejects the request.
///
/// Requires an `Idempotency-Key`: resubmitting with the same key — after a timeout, from
/// another device — returns the same request and never buys twice. Sales with a waiting room
/// also require a `Kippu-Admission-Pass`.
#[utoipa::path(
    post, path = "/v1/sales/{sale_id}/purchase-requests", tag = TAG,
    security(("bearer" = [])),
    params(
        ("sale_id" = SaleId, Path),
        ("Idempotency-Key" = String, Header, description = "Client-chosen key that makes retries safe"),
        ("Kippu-Admission-Pass" = Option<String>, Header, description = "Required for sales with a waiting room"),
    ),
    request_body = PurchaseRequestBody,
    responses(
        (status = 202, description = "Queued; poll the `Location`", body = PurchaseRequest),
        (status = 403, description = "No valid admission pass", body = Problem),
        (status = 409, description = "The sale is not open", body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn create_purchase_request(
    State(state): State<AppState>,
    principal: Principal,
    Path(sale_id): Path<SaleId>,
    headers: HeaderMap,
    Json(body): Json<PurchaseRequestBody>,
) -> ApiResult<Response> {
    let account = principal.require_account()?;
    state.authorize(&principal, PURCHASES_CREATE, Scope::Account(account))?;
    let key = header(&headers, IDEMPOTENCY_KEY)
        .ok_or(ValidationError::new(
            "Idempotency-Key",
            "header is required",
        ))
        .and_then(IdempotencyKey::new)?;
    let (sale, _) = visible_sale(&state, Some(&principal), sale_id).await?;
    let now = state.now();
    if !sale.is_open_at(now) {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "sale-closed",
            "this sale is not open",
        ));
    }
    if matches!(sale.admission, AdmissionPolicy::WaitingRoom { .. }) {
        let pass = header(&headers, ADMISSION_PASS).ok_or_else(|| {
            ApiError::new(
                StatusCode::FORBIDDEN,
                "admission-required",
                "join the waiting room first",
            )
        })?;
        state
            .tokens()
            .verify_admission_pass(pass, account, sale.id, now)?;
    }
    let basket = Basket::new(body.items)?;

    let request = submit(&state, account, &sale, &key, basket).await?;
    let location = format!("/v1/purchase-requests/{}", request.id);
    let mut response = (StatusCode::ACCEPTED, Json(request)).into_response();
    if let Ok(location) = HeaderValue::from_str(&location) {
        response.headers_mut().insert(LOCATION, location);
    }
    response.extensions_mut().insert(IdempotentByDesign);
    Ok(response)
}

/// Poll a purchase request: `queued`, `reserved` (with the reservation) or `rejected`.
#[utoipa::path(
    get, path = "/v1/purchase-requests/{purchase_request_id}", tag = TAG,
    security(("bearer" = [])),
    params(("purchase_request_id" = PurchaseRequestId, Path)),
    responses((status = 200, body = PurchaseRequest), (status = 404, body = Problem))
)]
pub(crate) async fn get_purchase_request(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<PurchaseRequestId>,
) -> ApiResult<Json<PurchaseRequest>> {
    let request = state
        .store()
        .purchase_request(id)
        .await?
        .filter(|request| {
            state.policy().permits(
                &principal,
                RESERVATIONS_READ,
                Scope::Account(request.account_id),
            )
        })
        .ok_or_else(|| ApiError::not_found("purchase request"))?;
    Ok(Json(request))
}

/// Your reservations, newest first.
#[utoipa::path(
    get, path = "/v1/me/reservations", tag = TAG,
    security(("bearer" = [])),
    responses((status = 200, body = Vec<Reservation>))
)]
pub(crate) async fn my_reservations(
    State(state): State<AppState>,
    principal: Principal,
) -> ApiResult<Json<Vec<Reservation>>> {
    let account = principal.require_account()?;
    state.authorize(&principal, RESERVATIONS_READ, Scope::Account(account))?;
    Ok(Json(state.store().reservations_for_account(account).await?))
}

/// A reservation.
#[utoipa::path(
    get, path = "/v1/reservations/{reservation_id}", tag = TAG,
    security(("bearer" = [])),
    params(("reservation_id" = ReservationId, Path)),
    responses((status = 200, body = Reservation), (status = 404, body = Problem))
)]
pub(crate) async fn get_reservation(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<ReservationId>,
) -> ApiResult<Json<Reservation>> {
    Ok(Json(own_reservation(&state, &principal, id).await?))
}

/// Choose how to pay. The chosen attestor is notified through `payment.requested`; free
/// reservations are issued immediately.
#[utoipa::path(
    post, path = "/v1/reservations/{reservation_id}/checkout", tag = TAG,
    security(("bearer" = [])),
    params(("reservation_id" = ReservationId, Path)),
    request_body = CheckoutRequest,
    responses((status = 200, body = Reservation), (status = 409, body = Problem), (status = 422, body = Problem))
)]
pub(crate) async fn checkout_reservation(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<ReservationId>,
    Json(request): Json<CheckoutRequest>,
) -> ApiResult<Json<Reservation>> {
    let reservation = own_reservation(&state, &principal, id).await?;
    Ok(Json(
        checkout(&state, &reservation, request.attestor_id).await?,
    ))
}

/// Give a reservation up, releasing its tickets.
#[utoipa::path(
    post, path = "/v1/reservations/{reservation_id}/cancel", tag = TAG,
    security(("bearer" = [])),
    params(("reservation_id" = ReservationId, Path)),
    responses((status = 200, body = Reservation), (status = 409, body = Problem))
)]
pub(crate) async fn cancel_reservation(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<ReservationId>,
) -> ApiResult<Json<Reservation>> {
    let reservation = own_reservation(&state, &principal, id).await?;
    Ok(Json(cancel(&state, &reservation).await?))
}
