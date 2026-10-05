use crate::http::Json;
use axum::extract::{Path, State};
use kippu_domain::TicketId;

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
    let ticket = state
        .store()
        .ticket(ticket_id)
        .await?
        .ok_or_else(|| ApiError::not_found("ticket"))?;
    if !state
        .policy()
        .permits(&principal, TICKETS_READ, Scope::Account(ticket.account_id))
    {
        return Err(ApiError::not_found("ticket"));
    }
    Ok(Json(TicketView::from(ticket)))
}
