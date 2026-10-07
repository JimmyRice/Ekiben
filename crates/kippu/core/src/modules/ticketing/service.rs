//! Ticket use cases, independent of HTTP: signing tickets for a paid reservation, and
//! handing them to their holders.

use std::collections::BTreeMap;

use kippu_domain::catalog::TicketType;
use kippu_domain::reservation::Reservation;
use kippu_domain::ticket::{Ticket, TicketStatus};
use kippu_domain::{TicketId, Timestamp};
use kippu_store::{Keyset, Page, PageRequest};

use super::permissions::TICKETS_READ;
use crate::app::AppState;
use crate::auth::{Principal, Scope};
use crate::error::{ApiError, ApiResult};
use crate::keys::PublishedKey;

/// What a gate needs to verify this deployment's tickets.
#[derive(Debug, Clone)]
pub struct GateKeys {
    /// The `issuer` claim tickets carry.
    pub issuer: String,
    /// The active key first, then retired keys still valid for older tickets.
    pub keys: Vec<PublishedKey>,
}

/// The keys tickets are signed with.
pub fn gate_keys(state: &AppState) -> GateKeys {
    GateKeys {
        issuer: state.config().issuer.id.clone(),
        keys: state.tickets().published(),
    }
}

/// The caller's tickets, most recently issued first.
#[tracing::instrument(skip_all)]
pub async fn tickets(
    state: &AppState,
    principal: &Principal,
    page: PageRequest<Keyset>,
) -> ApiResult<Page<Ticket, Keyset>> {
    let account = principal.require_account()?;
    state.authorize(principal, TICKETS_READ, Scope::Account(account))?;
    let tickets = state
        .store()
        .tickets_for_account(account, page.plus_one())
        .await?;
    Ok(Page::from_lookahead(tickets, page.limit, |ticket| Keyset {
        at: ticket.issued_at,
        id: ticket.id.as_uuid(),
    }))
}

/// A ticket of the caller's. Other people's tickets are reported as missing.
#[tracing::instrument(skip_all)]
pub async fn ticket(state: &AppState, principal: &Principal, id: TicketId) -> ApiResult<Ticket> {
    let ticket = state
        .store()
        .ticket(id)
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

fn unix_seconds(instant: Timestamp) -> u64 {
    u64::try_from(instant.unix_seconds()).unwrap_or(0)
}

/// Signs one ticket per reserved seat. Pure computation: the caller stores the tickets in the
/// same transaction that records the payment.
pub(crate) fn issue_tickets(
    state: &AppState,
    reservation: &Reservation,
    ticket_types: &[TicketType],
    now: Timestamp,
) -> ApiResult<Vec<Ticket>> {
    let issuer = state.tickets().issuer();
    let mut tickets = Vec::new();
    for item in &reservation.items {
        let ticket_type = ticket_types
            .iter()
            .find(|ticket_type| ticket_type.id == item.ticket_type_id)
            .ok_or_else(|| ApiError::internal("reserved ticket type no longer exists"))?;
        let extensions: BTreeMap<u8, Vec<u8>> = ticket_type
            .ticket_extensions
            .iter()
            .map(|(&tag, value)| (tag, value.as_bytes().to_vec()))
            .collect();
        for _ in 0..item.quantity {
            let id = TicketId::generate();
            let claims = kaisatsu::Claims {
                issuer: state.config().issuer.id.clone(),
                event_id: kaisatsu::Uuid::from_bytes(reservation.event_id.as_uuid().into_bytes()),
                ticket_id: kaisatsu::Uuid::from_bytes(id.as_uuid().into_bytes()),
                ticket_type_id: kaisatsu::Uuid::from_bytes(ticket_type.id.as_uuid().into_bytes()),
                valid_from: unix_seconds(ticket_type.valid_from),
                valid_until: unix_seconds(ticket_type.valid_until),
                issued_at: unix_seconds(now),
                extensions: extensions.clone(),
            };
            let encoded = issuer.issue(&claims).map_err(ApiError::internal)?;
            tickets.push(Ticket {
                id,
                reservation_id: reservation.id,
                account_id: reservation.account_id,
                event_id: reservation.event_id,
                ticket_type_id: ticket_type.id,
                valid_from: ticket_type.valid_from,
                valid_until: ticket_type.valid_until,
                issued_at: now,
                status: TicketStatus::Valid,
                encoded,
            });
        }
    }
    Ok(tickets)
}
