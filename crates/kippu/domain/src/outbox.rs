//! Integration events: facts other systems may want to react to.
//!
//! They are written to an outbox in the same transaction as the change they describe, so an
//! event exists if and only if the change happened. Delivery is at-least-once; consumers must
//! be idempotent (each event carries a unique, increasing sequence number).

use serde::{Deserialize, Serialize};

use crate::refund::RefundReason;
use crate::{AccountId, AttestorId, EventId, Money, RefundId, ReservationId, TicketId, Timestamp};

/// Something that happened, for consumers outside Kippu.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
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
    /// Money must be returned: an attested payment could not be used, or tickets it paid for
    /// were refunded.
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
        /// The refund of issued tickets this is for; absent when the whole payment could not
        /// be used. Confirm with it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        refund_id: Option<RefundId>,
    },
    /// Tickets were revoked: refunded, or their payment reversed. Gates must refuse them.
    #[serde(rename = "tickets.revoked")]
    TicketsRevoked {
        /// The reservation the tickets were bought with.
        reservation_id: ReservationId,
        /// The event they admitted to.
        event_id: EventId,
        /// The revoked tickets.
        ticket_ids: Vec<TicketId>,
        /// Why.
        reason: RefundReason,
    },
    /// An unpaid reservation lapsed and its inventory was released.
    #[serde(rename = "reservation.expired")]
    ReservationExpired {
        /// The lapsed reservation.
        reservation_id: ReservationId,
    },
}

impl IntegrationEvent {
    /// Every topic there is.
    pub const TOPICS: [&'static str; 5] = [
        "payment.requested",
        "tickets.issued",
        "refund.required",
        "reservation.expired",
        "tickets.revoked",
    ];

    /// The reservation the event is about. Every event concerns one.
    pub const fn reservation_id(&self) -> ReservationId {
        match self {
            Self::PaymentRequested { reservation_id, .. }
            | Self::TicketsIssued { reservation_id, .. }
            | Self::RefundRequired { reservation_id, .. }
            | Self::ReservationExpired { reservation_id }
            | Self::TicketsRevoked { reservation_id, .. } => *reservation_id,
        }
    }

    /// The event's topic, e.g. `"payment.requested"`.
    pub const fn topic(&self) -> &'static str {
        match self {
            Self::PaymentRequested { .. } => "payment.requested",
            Self::TicketsIssued { .. } => "tickets.issued",
            Self::RefundRequired { .. } => "refund.required",
            Self::ReservationExpired { .. } => "reservation.expired",
            Self::TicketsRevoked { .. } => "tickets.revoked",
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
        assert!(IntegrationEvent::TOPICS.contains(&event.topic()));
    }
}
