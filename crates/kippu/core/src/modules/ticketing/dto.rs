use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use kippu_domain::ticket::{Ticket, TicketStatus};
use kippu_domain::{EventId, ReservationId, TicketId, TicketTypeId, Timestamp};
use serde::Serialize;
use utoipa::ToSchema;

use super::service::GateKeys;
use crate::keys::PublishedKey;

/// A ticket and its signed bytes.
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
    /// The signed KP1 ticket, standard Base64 (RFC 4648, padded). Decode it and carry the
    /// bytes to the gate however suits you — a binary QR code, Base45, or this text as is; see
    /// `spec/ticket-protocol.md`. `GET /v1/tickets/{ticket_id}/raw` returns the bytes directly.
    #[schema(format = Byte, example = "S1ABAQ…")]
    pub ticket: String,
}

impl From<Ticket> for TicketView {
    fn from(ticket: Ticket) -> Self {
        Self {
            ticket: STANDARD.encode(&ticket.encoded),
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

impl From<GateKeys> for TicketKeys {
    fn from(keys: GateKeys) -> Self {
        Self {
            issuer: keys.issuer,
            keys: keys.keys,
        }
    }
}
