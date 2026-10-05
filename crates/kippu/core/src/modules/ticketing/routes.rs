use crate::http::Json;
use axum::extract::{Path, State};
use axum::http::HeaderValue;
use axum::http::header::CONTENT_TYPE;
use axum::response::IntoResponse;
use kippu_domain::TicketId;
use kippu_domain::ticket::Ticket;

use super::dto::{TicketKeys, TicketView};
use super::permissions::TICKETS_READ;
use crate::app::AppState;
use crate::auth::{Principal, Scope};
use crate::error::{ApiError, ApiResult, Problem};

const TAG: &str = "ticketing";

/// The public keys tickets are signed with. Gates load them into Kaisatsu.
#[utoipa::path(
    get, path = "/.well-known/kippu/ticket-keys", tag = TAG,
    responses((status = 200, body = TicketKeys))
)]
pub(crate) async fn ticket_keys(State(state): State<AppState>) -> Json<TicketKeys> {
    Json(TicketKeys {
        issuer: state.config().issuer.id.clone(),
        keys: state.tickets().published(),
    })
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
    let account = principal.require_account()?;
    state.authorize(&principal, TICKETS_READ, Scope::Account(account))?;
    let tickets = state.store().tickets_for_account(account).await?;
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
        own_ticket(&state, &principal, ticket_id).await?,
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
    let ticket = own_ticket(&state, &principal, ticket_id).await?;
    Ok((
        [(
            CONTENT_TYPE,
            HeaderValue::from_static("application/octet-stream"),
        )],
        ticket.encoded,
    ))
}

/// A ticket of the caller's. Other people's tickets are reported as missing.
async fn own_ticket(
    state: &AppState,
    principal: &Principal,
    ticket_id: TicketId,
) -> ApiResult<Ticket> {
    let ticket = state
        .store()
        .ticket(ticket_id)
        .await?
        .ok_or_else(|| ApiError::not_found("ticket"))?;
    if !state
        .policy()
        .permits(principal, TICKETS_READ, Scope::Account(ticket.account_id))
    {
        return Err(ApiError::not_found("ticket"));
    }
    Ok(ticket)
}
