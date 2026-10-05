use kippu_domain::ticket::{Ticket, TicketStatus};
use kippu_domain::{EventId, ReservationId, TicketId, TicketTypeId, Timestamp};
use serde::Serialize;
use utoipa::ToSchema;

use crate::keys::PublishedKey;

/// A ticket, ready to show as a QR code.
#[derive(Debug, Serialize, ToSchema)]
pub struct TicketView {
    /// Identity of the ticket.
    pub id: TicketId,
    /// The paid reservation it came from.
    pub reservation_id: ReservationId,
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
    /// The signed ticket as Base45 text: put this in a QR code (alphanumeric mode).
    pub qr: String,
}

impl From<Ticket> for TicketView {
    fn from(ticket: Ticket) -> Self {
        Self {
            qr: kaisatsu::base45::encode(&ticket.encoded),
            id: ticket.id,
            reservation_id: ticket.reservation_id,
            event_id: ticket.event_id,
            ticket_type_id: ticket.ticket_type_id,
            valid_from: ticket.valid_from,
            valid_until: ticket.valid_until,
            issued_at: ticket.issued_at,
            status: ticket.status,
        }
    }
}

/// The keys a gate should trust.
#[derive(Debug, Serialize, ToSchema)]
pub struct TicketKeys {
    /// The `issuer` claim tickets from this deployment carry.
    pub issuer: String,
    /// The active key first, then retired keys still valid for older tickets.
    pub keys: Vec<PublishedKey>,
}
