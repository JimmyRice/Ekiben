//! HTTP handlers: each turns a request into one [`service`](super::service) call.

use axum::extract::{Path, State};
use axum::http::HeaderValue;
use axum::http::header::CONTENT_TYPE;
use axum::response::IntoResponse;
use kippu_domain::TicketId;

use super::dto::{TicketKeys, TicketView};
use super::service;
use crate::app::AppState;
use crate::auth::Principal;
use crate::error::{ApiResult, Problem};
use crate::http::Json;

const TAG: &str = "ticketing";

/// The public keys tickets are signed with. Gates load them into Kaisatsu.
#[utoipa::path(
    get, path = "/.well-known/kippu/ticket-keys", tag = TAG,
    responses((status = 200, body = TicketKeys))
)]
pub(crate) async fn ticket_keys(State(state): State<AppState>) -> Json<TicketKeys> {
    Json(service::gate_keys(&state).into())
}

/// Your tickets, newest first.
#[utoipa::path(
    get, path = "/v1/me/tickets", tag = TAG,
    security(("bearer" = [])),
    responses((status = 200, body = Vec<TicketView>))
)]
pub(crate) async fn my_tickets(
    State(state): State<AppState>,
    principal: Principal,
) -> ApiResult<Json<Vec<TicketView>>> {
    let tickets = service::tickets(&state, &principal).await?;
    Ok(Json(tickets.into_iter().map(TicketView::from).collect()))
}

/// A ticket.
#[utoipa::path(
    get, path = "/v1/tickets/{ticket_id}", tag = TAG,
    security(("bearer" = [])),
    params(("ticket_id" = TicketId, Path)),
    responses((status = 200, body = TicketView), (status = 404, body = Problem))
)]
pub(crate) async fn get_ticket(
    State(state): State<AppState>,
    principal: Principal,
    Path(ticket_id): Path<TicketId>,
) -> ApiResult<Json<TicketView>> {
    Ok(Json(TicketView::from(
        service::ticket(&state, &principal, ticket_id).await?,
    )))
}

/// A ticket's signed KP1 bytes, as they are to reach the gate.
#[utoipa::path(
    get, path = "/v1/tickets/{ticket_id}/raw", tag = TAG,
    security(("bearer" = [])),
    params(("ticket_id" = TicketId, Path)),
    responses(
        (status = 200, description = "The KP1 ticket", content_type = "application/octet-stream", body = Vec<u8>),
        (status = 404, body = Problem)
    )
)]
pub(crate) async fn get_ticket_raw(
    State(state): State<AppState>,
    principal: Principal,
    Path(ticket_id): Path<TicketId>,
) -> ApiResult<impl IntoResponse> {
    let ticket = service::ticket(&state, &principal, ticket_id).await?;
    Ok((
        [(
            CONTENT_TYPE,
            HeaderValue::from_static("application/octet-stream"),
        )],
        ticket.encoded,
    ))
}
