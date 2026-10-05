//! Integration events: facts other systems may want to react to.
//!
//! They are written to an outbox in the same transaction as the change they describe, so an
//! event exists if and only if the change happened. Delivery is at-least-once; consumers must
//! be idempotent (each event carries a unique, increasing sequence number).

use serde::{Deserialize, Serialize};

use crate::{AccountId, AttestorId, Money, ReservationId, TicketId, Timestamp};

/// Something that happened, for consumers outside Kippu.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "topic")]
pub enum IntegrationEvent {
    /// A buyer chose an attestor to pay a reservation with.
    #[serde(rename = "payment.requested")]
    PaymentRequested {
        /// The reservation to pay for.
        reservation_id: ReservationId,
        /// The attestor the buyer chose.
        attestor_id: AttestorId,
        /// The exact amount to charge.
        amount: Money,
        /// Payment must be attested before this instant.
        expires_at: Timestamp,
    },
    /// Tickets were issued for a paid reservation.
    #[serde(rename = "tickets.issued")]
    TicketsIssued {
        /// The paid reservation.
        reservation_id: ReservationId,
        /// The ticket holder.
        account_id: AccountId,
        /// The new tickets.
        ticket_ids: Vec<TicketId>,
    },
    /// An attested payment could not be used and must be returned.
    #[serde(rename = "refund.required")]
    RefundRequired {
        /// The reservation the payment was for.
        reservation_id: ReservationId,
        /// The attestor that must refund.
        attestor_id: AttestorId,
        /// The attestor's reference for the payment.
        attestation_id: String,
        /// How much to refund.
        amount: Money,
    },
    /// An unpaid reservation lapsed and its inventory was released.
    #[serde(rename = "reservation.expired")]
    ReservationExpired {
        /// The lapsed reservation.
        reservation_id: ReservationId,
    },
}

impl IntegrationEvent {
    /// The event's topic, e.g. `"payment.requested"`.
    pub const fn topic(&self) -> &'static str {
        match self {
            Self::PaymentRequested { .. } => "payment.requested",
            Self::TicketsIssued { .. } => "tickets.issued",
            Self::RefundRequired { .. } => "refund.required",
            Self::ReservationExpired { .. } => "reservation.expired",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_topic_tag_matches_topic() {
        let event = IntegrationEvent::ReservationExpired {
            reservation_id: ReservationId::generate(),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["topic"], event.topic());
    }
}
