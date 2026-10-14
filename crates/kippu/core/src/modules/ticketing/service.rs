//! Ticket use cases, independent of HTTP: signing tickets for a paid reservation, handing
//! them to their holders, and showing them to the event's organizers (without the signed
//! bytes, which only the holder gets).

use std::collections::BTreeMap;

use kippu_domain::catalog::TicketType;
use kippu_domain::reservation::Reservation;
use kippu_domain::ticket::{Ticket, TicketStatus};
use kippu_domain::{EventId, ReservationId, TicketId, TicketTypeId, Timestamp, ValidationError};
use kippu_store::{Keyset, Page, PageRequest};

use super::permissions::TICKETS_READ;
use crate::app::AppState;
use crate::auth::{Principal, Scope};
use crate::error::{ApiError, ApiResult};
use crate::keys::PublishedKey;
use crate::modules::purchasing::service::{reads_orders_of, reservation};

/// What a gate needs to verify this deployment's tickets.
#[derive(Debug, Clone)]
pub struct GateKeys {
    /// The `issuer` claim tickets carry.
    pub issuer: String,
    /// The active key first, then retired keys still valid for older tickets.
    pub keys: Vec<PublishedKey>,
}

/// A ticket as the caller sees it: the ticket, and the name of its type.
#[derive(Debug, Clone)]
pub struct TicketDetails {
    /// The ticket.
    pub ticket: Ticket,
    /// The ticket type's current name, e.g. "Day 1".
    pub ticket_type_name: String,
    /// Whether the caller holds the ticket (or is an admin): only then may they have its
    /// signed bytes. An organizer of the event sees everything else.
    pub holder: bool,
}

/// The keys tickets are signed with.
pub fn gate_keys(state: &AppState) -> GateKeys {
    GateKeys {
        issuer: state.config().issuer.id.to_string(),
        keys: state.tickets().published(),
    }
}

/// The caller's tickets, all of them or those for `event`, most recently issued first.
#[tracing::instrument(skip_all)]
pub async fn tickets(
    state: &AppState,
    principal: &Principal,
    event: Option<EventId>,
    page: PageRequest<Keyset>,
) -> ApiResult<Page<TicketDetails, Keyset>> {
    let account = principal.require_account()?;
    state.authorize(principal, TICKETS_READ, Scope::Account(account))?;
    let tickets = state
        .store()
        .tickets_for_account(account, event, page.plus_one())
        .await?;
    let page = Page::from_lookahead(tickets, page.limit, |ticket| Keyset {
        at: ticket.issued_at,
        id: ticket.id.as_uuid(),
    });
    Ok(Page {
        items: with_type_names(state, page.items, true).await?,
        next: page.next,
    })
}

/// The tickets issued for a reservation the caller may see: their own, or one for an event
/// they organize.
#[tracing::instrument(skip_all)]
pub async fn reservation_tickets(
    state: &AppState,
    principal: &Principal,
    id: ReservationId,
) -> ApiResult<Vec<TicketDetails>> {
    let reservation = reservation(state, principal, id).await?;
    let holder = state.policy().permits(
        principal,
        TICKETS_READ,
        Scope::Account(reservation.account_id),
    );
    let tickets = state
        .store()
        .tickets_for_reservation(reservation.id)
        .await?;
    with_type_names(state, tickets, holder).await
}

/// Adds the names of their types to tickets. A page holds few ticket types: each is looked up
/// once.
async fn with_type_names(
    state: &AppState,
    tickets: Vec<Ticket>,
    holder: bool,
) -> ApiResult<Vec<TicketDetails>> {
    let mut names: BTreeMap<TicketTypeId, String> = BTreeMap::new();
    let mut items = Vec::with_capacity(tickets.len());
    for ticket in tickets {
        let name = if let Some(name) = names.get(&ticket.ticket_type_id) {
            name.clone()
        } else {
            let name = ticket_type_name(state, ticket.ticket_type_id).await?;
            names.insert(ticket.ticket_type_id, name.clone());
            name
        };
        items.push(TicketDetails {
            ticket,
            ticket_type_name: name,
            holder,
        });
    }
    Ok(items)
}

/// A ticket of the caller's, with its signed bytes. Other people's tickets are reported as
/// missing, also to the event's organizers.
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

/// A ticket the caller may see, with the name of its type: their own, or one for an event
/// they organize. Others are reported as missing.
#[tracing::instrument(skip_all)]
pub async fn ticket_details(
    state: &AppState,
    principal: &Principal,
    id: TicketId,
) -> ApiResult<TicketDetails> {
    let ticket = state
        .store()
        .ticket(id)
        .await?
        .ok_or_else(|| ApiError::not_found("ticket"))?;
    let holder = state
        .policy()
        .permits(principal, TICKETS_READ, Scope::Account(ticket.account_id));
    if !holder && !reads_orders_of(state, principal, ticket.event_id).await? {
        return Err(ApiError::not_found("ticket"));
    }
    let ticket_type_name = ticket_type_name(state, ticket.ticket_type_id).await?;
    Ok(TicketDetails {
        ticket,
        ticket_type_name,
        holder,
    })
}

/// The current name of a ticket type. Ticket types are never deleted, so an issued ticket's
/// type always exists.
async fn ticket_type_name(state: &AppState, id: TicketTypeId) -> ApiResult<String> {
    state
        .store()
        .ticket_type(id)
        .await?
        .map(|ticket_type| ticket_type.name)
        .ok_or_else(|| ApiError::internal("an issued ticket's type no longer exists"))
}

fn unix_seconds(instant: Timestamp) -> u64 {
    u64::try_from(instant.unix_seconds()).unwrap_or(0)
}

/// The claims of a ticket of `ticket_type`, with a placeholder ticket id.
fn claims(
    state: &AppState,
    event_id: EventId,
    ticket_type: &TicketType,
    now: Timestamp,
) -> kaisatsu::Claims {
    let uuid = |id: uuid::Uuid| kaisatsu::Uuid::from_bytes(id.into_bytes());
    kaisatsu::Claims {
        issuer: state.config().issuer.id.to_string(),
        event_id: uuid(event_id.as_uuid()),
        ticket_id: kaisatsu::Uuid::from_bytes([0; 16]),
        ticket_type_id: uuid(ticket_type.id.as_uuid()),
        valid_from: unix_seconds(ticket_type.valid_from),
        valid_until: unix_seconds(ticket_type.valid_until),
        issued_at: unix_seconds(now),
        extensions: ticket_type
            .ticket_extensions
            .iter()
            .map(|(&tag, value)| (tag, value.as_bytes().to_vec()))
            .collect(),
    }
}

/// Checks that tickets of `ticket_type` can be issued: its extension claims must leave them
/// within the ticket length limit. Every ticket of a type has the same length.
pub(crate) fn check_ticket_length(state: &AppState, ticket_type: &TicketType) -> ApiResult<()> {
    let claims = claims(
        state,
        ticket_type.event_id,
        ticket_type,
        Timestamp::UNIX_EPOCH,
    );
    if claims.encoded_len() > kaisatsu::wire::MAX_TICKET_LEN {
        return Err(ValidationError::new(
            "ticket_extensions",
            "make tickets longer than 2048 bytes",
        )
        .into());
    }
    Ok(())
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
        // Tickets of one type differ only in their id.
        let mut claims = claims(state, reservation.event_id, ticket_type, now);
        for _ in 0..item.quantity {
            let id = TicketId::generate();
            claims.ticket_id = kaisatsu::Uuid::from_bytes(id.as_uuid().into_bytes());
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
