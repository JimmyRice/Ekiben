//! Issued tickets.

use serde::{Deserialize, Serialize};

use crate::{AccountId, EventId, ReservationId, TicketId, TicketTypeId, Timestamp};

/// Whether a ticket is still honoured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum TicketStatus {
    /// Honoured.
    Valid,
    /// Withdrawn, e.g. after a refund. Gates learn about revocations out of band.
    Revoked,
}

/// A ticket issued for a paid reservation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ticket {
    /// Identity of the ticket, also its KP1 `ticket_id` claim.
    pub id: TicketId,
    /// The reservation it was issued for.
    pub reservation_id: ReservationId,
    /// The holder.
    pub account_id: AccountId,
    /// The event it admits to.
    pub event_id: EventId,
    /// Its ticket type.
    pub ticket_type_id: TicketTypeId,
    /// Valid from (inclusive).
    pub valid_from: Timestamp,
    /// Valid until (exclusive).
    pub valid_until: Timestamp,
    /// When it was issued.
    pub issued_at: Timestamp,
    /// Whether it is honoured.
    pub status: TicketStatus,
    /// The signed KP1 bytes, exactly as issued. Kept so key rotation never changes a ticket.
    pub encoded: Vec<u8>,
}
