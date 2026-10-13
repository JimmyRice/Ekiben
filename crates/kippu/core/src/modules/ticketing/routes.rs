//! HTTP handlers: each turns a request into one [`service`](super::service) call.

use axum::extract::{Path, Query, State};
use axum::http::HeaderValue;
use axum::http::header::CONTENT_TYPE;
use axum::response::IntoResponse;
use kippu_domain::{ReservationId, TicketId};

use super::dto::{TicketKeys, TicketView};
use super::service;
use crate::app::AppState;
use crate::auth::Principal;
use crate::error::{ApiResult, Problem};
use crate::http::{Json, Listing, ListingTag, PageQuery};

const TAG: &str = "ticketing";

/// The public keys tickets are signed with. Gates load them into Kaisatsu.
#[utoipa::path(
    get, path = "/.well-known/kippu/ticket-keys", tag = TAG,
    responses((status = 200, body = TicketKeys))
)]
pub(crate) async fn ticket_keys(State(state): State<AppState>) -> Json<TicketKeys> {
    Json(service::gate_keys(&state).into())
}

/// Your tickets, most recently issued first.
#[utoipa::path(
    get, path = "/v1/me/tickets", tag = TAG,
    security(("bearer" = [])),
    params(PageQuery),
    responses((status = 200, body = Listing<TicketView>), (status = 400, body = Problem))
)]
pub(crate) async fn my_tickets(
    State(state): State<AppState>,
    principal: Principal,
    Query(query): Query<PageQuery>,
) -> ApiResult<Json<Listing<TicketView>>> {
    let page = query.page(ListingTag::TICKETS)?;
    let tickets = service::tickets(&state, &principal, page).await?;
    Ok(Json(Listing::page(
        tickets,
        ListingTag::TICKETS,
        TicketView::from,
    )))
}

/// The tickets issued for a reservation, in id order: your own, or, for organizers, one for
/// their event.
#[utoipa::path(
    get, path = "/v1/reservations/{reservation_id}/tickets", tag = TAG,
    security(("bearer" = [])),
    params(("reservation_id" = ReservationId, Path)),
    responses((status = 200, body = Listing<TicketView>), (status = 404, body = Problem))
)]
pub(crate) async fn list_reservation_tickets(
    State(state): State<AppState>,
    principal: Principal,
    Path(reservation_id): Path<ReservationId>,
) -> ApiResult<Json<Listing<TicketView>>> {
    let tickets = service::reservation_tickets(&state, &principal, reservation_id).await?;
    Ok(Json(Listing::all(
        tickets.into_iter().map(TicketView::from).collect(),
    )))
}

/// A ticket: your own, or, for organizers, one for their event (without its signed bytes).
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
        service::ticket_details(&state, &principal, ticket_id).await?,
    )))
}

/// A ticket's signed KP1 bytes, as they are to reach the gate. Only for its holder.
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
