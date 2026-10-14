//! HTTP handlers: each turns a request into one [`service`](super::service) call.

use axum::extract::{Path, State};
use kippu_domain::SaleId;

use kippu_domain::admission::WaitingRoom;

use super::dto::{AdmissionRequest, AdmissionView, QueuePlace};
use super::service;
use crate::app::AppState;
use crate::auth::Principal;
use crate::error::{ApiResult, Problem};
use crate::http::Json;

const TAG: &str = "admission";

/// How far a sale's waiting room has got, for its organizers: `last_position` handed out,
/// everyone through `admitted_through` let in.
#[utoipa::path(
    get, path = "/v1/sales/{sale_id}/waiting-room", tag = TAG,
    security(("bearer" = [])),
    params(("sale_id" = SaleId, Path)),
    responses(
        (status = 200, body = WaitingRoom),
        (status = 403, body = Problem),
        (status = 404, body = Problem),
        (status = 409, description = "The sale has no waiting room", body = Problem)
    )
)]
pub(crate) async fn get_waiting_room(
    State(state): State<AppState>,
    principal: Principal,
    Path(sale_id): Path<SaleId>,
) -> ApiResult<Json<WaitingRoom>> {
    Ok(Json(
        service::waiting_room(&state, &principal, sale_id).await?,
    ))
}

/// Join a sale's waiting room.
#[utoipa::path(
    post, path = "/v1/sales/{sale_id}/waiting-room", tag = TAG,
    security(("bearer" = [])),
    params(("sale_id" = SaleId, Path)),
    responses((status = 200, body = QueuePlace), (status = 409, description = "The sale has no waiting room", body = Problem))
)]
pub(crate) async fn join_waiting_room(
    State(state): State<AppState>,
    principal: Principal,
    Path(sale_id): Path<SaleId>,
) -> ApiResult<Json<QueuePlace>> {
    let ticket = service::join_waiting_room(&state, &principal, sale_id).await?;
    Ok(Json(ticket.into()))
}

/// Ask whether it is your turn to buy. Sales without a waiting room admit everyone.
#[utoipa::path(
    post, path = "/v1/sales/{sale_id}/admission", tag = TAG,
    security(("bearer" = [])),
    params(("sale_id" = SaleId, Path)),
    request_body = AdmissionRequest,
    responses((status = 200, body = AdmissionView), (status = 403, body = Problem))
)]
pub(crate) async fn request_admission(
    State(state): State<AppState>,
    principal: Principal,
    Path(sale_id): Path<SaleId>,
    Json(request): Json<AdmissionRequest>,
) -> ApiResult<Json<AdmissionView>> {
    let admission =
        service::request_admission(&state, &principal, sale_id, request.queue_ticket).await?;
    Ok(Json(admission.into()))
}
