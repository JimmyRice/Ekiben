//! HTTP handlers: headers, `Location` and status codes are decided here; everything about
//! buying is the [`service`](super::service)'s.

use axum::extract::{Path, Query, State};
use axum::http::header::LOCATION;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use kippu_domain::purchase::PurchaseRequest;
use kippu_domain::reservation::Reservation;
use kippu_domain::{PurchaseRequestId, ReservationId, SaleId};

use super::dto::{CheckoutRequest, PollQuery, PurchaseRequestBody};
use super::service::{self, PurchaseSubmission};
use crate::app::AppState;
use crate::auth::Principal;
use crate::error::{ApiResult, Problem};
use crate::http::idempotency::{IDEMPOTENCY_KEY, IdempotentByDesign};
use crate::http::{Json, Listing, ListingTag, PageQuery};
use crate::modules::admission::ADMISSION_PASS;

const TAG: &str = "purchasing";

fn header(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
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
    let submission = PurchaseSubmission {
        idempotency_key: header(&headers, IDEMPOTENCY_KEY),
        admission_pass: header(&headers, ADMISSION_PASS),
        items: body.items,
    };
    let submitted = service::submit_purchase(&state, &principal, sale_id, submission).await?;
    let mut location = format!("/v1/purchase-requests/{}", submitted.request.id);
    if let Some(receipt) = submitted.receipt {
        location = format!("{location}?receipt={receipt}");
    }
    let mut response = (StatusCode::ACCEPTED, Json(submitted.request)).into_response();
    if let Ok(location) = HeaderValue::from_str(&location) {
        response.headers_mut().insert(LOCATION, location);
    }
    response.extensions_mut().insert(IdempotentByDesign);
    Ok(response)
}

/// Poll a purchase request: `queued`, `reserved` (with the reservation) or `rejected`.
/// Use the `Location` from submitting it as is.
#[utoipa::path(
    get, path = "/v1/purchase-requests/{purchase_request_id}", tag = TAG,
    security(("bearer" = [])),
    params(("purchase_request_id" = PurchaseRequestId, Path), PollQuery),
    responses((status = 200, body = PurchaseRequest), (status = 404, body = Problem))
)]
pub(crate) async fn get_purchase_request(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<PurchaseRequestId>,
    Query(query): Query<PollQuery>,
) -> ApiResult<Json<PurchaseRequest>> {
    Ok(Json(
        service::purchase_request(&state, &principal, id, query.receipt).await?,
    ))
}

/// Your reservations, newest first.
#[utoipa::path(
    get, path = "/v1/me/reservations", tag = TAG,
    security(("bearer" = [])),
    params(PageQuery),
    responses((status = 200, body = Listing<Reservation>), (status = 400, body = Problem))
)]
pub(crate) async fn my_reservations(
    State(state): State<AppState>,
    principal: Principal,
    Query(query): Query<PageQuery>,
) -> ApiResult<Json<Listing<Reservation>>> {
    let page = query.page(ListingTag::RESERVATIONS)?;
    let reservations = service::reservations(&state, &principal, page).await?;
    Ok(Json(Listing::page(
        reservations,
        ListingTag::RESERVATIONS,
        |reservation| reservation,
    )))
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
    Ok(Json(service::reservation(&state, &principal, id).await?))
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
    Ok(Json(
        service::checkout(&state, &principal, id, request.attestor_id).await?,
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
    Ok(Json(service::cancel(&state, &principal, id).await?))
}
