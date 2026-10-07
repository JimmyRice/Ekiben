//! Refunds of issued tickets.
//!
//! A refund starts at Kippu: the buyer (within the ticket type's refund period) or an organizer
//! asks for it, and in one transaction the tickets are revoked, their stock returns to the
//! pool and an obligation goes to the attestor that took the payment. The attestor returns
//! the money and confirms; only then is the refund complete.
//!
//! ```text
//! requested ──tickets revoked, stock returned──▶ Pending ──attestor confirms──▶ Completed
//! ```
//!
//! An attestor can also report that money went back without Kippu asking — a chargeback, or a
//! refund made in the provider's dashboard. That *reversal* revokes the tickets it paid for and
//! is complete at once: the money is already gone.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{AccountId, AttestorId, EventId, Money, RefundId, ReservationId, TicketId, Timestamp};

impl RefundId {
    /// The id of the reversal of a payment: the same for every delivery of the report, so
    /// it is recorded once.
    pub fn reversal(attestor: AttestorId, attestation_id: &str) -> Self {
        let digest = Sha256::new()
            .chain_update(b"kippu/refund-reversal/v1\0")
            .chain_update(attestor.as_uuid().as_bytes())
            .chain_update(attestation_id.as_bytes())
            .finalize();
        let mut bytes = [0; 16];
        bytes.copy_from_slice(digest.get(..16).unwrap_or(&[0; 16]));
        Self::from_uuid(Uuid::new_v8(bytes))
    }
}

/// Why the tickets were revoked and the money returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum RefundReason {
    /// The buyer or an organizer asked Kippu for it.
    Requested,
    /// The attestor reported that the money went back on its own: a chargeback, or a refund
    /// made outside Kippu.
    Reversal,
}

impl RefundReason {
    /// The reason's name as stored and serialized.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Requested => "requested",
            Self::Reversal => "reversal",
        }
    }
}

impl std::str::FromStr for RefundReason {
    type Err = crate::ValidationError;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        match name {
            "requested" => Ok(Self::Requested),
            "reversal" => Ok(Self::Reversal),
            _ => Err(crate::ValidationError::new(
                "reason",
                "is not a known refund reason",
            )),
        }
    }
}

/// Where a refund stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum RefundStatus {
    /// The tickets are revoked; the attestor has yet to confirm it returned the money.
    Pending,
    /// The money is back with the buyer (or there was none to return).
    Completed,
}

impl RefundStatus {
    /// The status's name as stored and serialized.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Completed => "completed",
        }
    }
}

impl std::str::FromStr for RefundStatus {
    type Err = crate::ValidationError;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        match name {
            "pending" => Ok(Self::Pending),
            "completed" => Ok(Self::Completed),
            _ => Err(crate::ValidationError::new(
                "status",
                "is not a known refund status",
            )),
        }
    }
}

/// Money returned for some of a reservation's tickets, which are revoked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Refund {
    /// Identity of the refund.
    pub id: RefundId,
    /// The reservation the tickets were bought with.
    pub reservation_id: ReservationId,
    /// The buyer, who gets the money back.
    pub account_id: AccountId,
    /// The event the tickets admitted to.
    pub event_id: EventId,
    /// The attestor that took the payment and returns the money.
    pub attestor_id: AttestorId,
    /// The attestor's reference for the payment being returned.
    pub attestation_id: String,
    /// The tickets revoked by this refund.
    pub ticket_ids: Vec<TicketId>,
    /// How much goes back: the price the revoked tickets were bought at.
    pub amount: Money,
    /// Why.
    pub reason: RefundReason,
    /// Whether the money is back yet.
    pub status: RefundStatus,
    /// When the refund was recorded (and the tickets revoked).
    pub created_at: Timestamp,
    /// When the attestor confirmed the money was returned.
    pub completed_at: Option<Timestamp>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reversal_ids_are_stable_per_payment() {
        let attestor = AttestorId::generate();
        assert_eq!(
            RefundId::reversal(attestor, "pi_1"),
            RefundId::reversal(attestor, "pi_1")
        );
        assert_ne!(
            RefundId::reversal(attestor, "pi_1"),
            RefundId::reversal(attestor, "pi_2")
        );
        assert_ne!(
            RefundId::reversal(attestor, "pi_1"),
            RefundId::reversal(AttestorId::generate(), "pi_1")
        );
    }

    #[test]
    fn names_round_trip() {
        for reason in [RefundReason::Requested, RefundReason::Reversal] {
            assert_eq!(reason.as_str().parse::<RefundReason>(), Ok(reason));
        }
        for status in [RefundStatus::Pending, RefundStatus::Completed] {
            assert_eq!(status.as_str().parse::<RefundStatus>(), Ok(status));
        }
    }
}
