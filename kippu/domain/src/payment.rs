//! Payment attestation: how Kippu learns that money really arrived.
//!
//! Kippu knows nothing about Apple Pay, Alipay or Stripe. A deployment registers *attestors* —
//! small services it trusts, each holding an Ed25519 key — and an attestor signs a statement
//! that a reservation has been paid. Adding or removing a payment method means registering
//! or revoking an attestor at runtime; Kippu's code never changes.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{AttestorId, Money, ReservationId, Timestamp, ValidationError};

impl AttestorId {
    /// Built-in attestor that settles free reservations at checkout.
    pub const FREE: Self = Self::from_uuid(Uuid::from_u128(1));
    /// Built-in attestor for payments taken in person (e.g. cash at the venue), recorded by an
    /// organizer or admin.
    pub const MANUAL: Self = Self::from_uuid(Uuid::from_u128(2));

    /// Whether this is one of the built-in attestors, which have no signing keys.
    pub fn is_builtin(self) -> bool {
        self == Self::FREE || self == Self::MANUAL
    }
}

/// Whether real money is involved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Environment {
    /// Real payments.
    Live,
    /// Test payments. A sandbox attestor can never settle a live sale.
    Sandbox,
}

impl Environment {
    /// The environment's name as stored and serialized.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Sandbox => "sandbox",
        }
    }
}

impl std::str::FromStr for Environment {
    type Err = ValidationError;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        match name {
            "live" => Ok(Self::Live),
            "sandbox" => Ok(Self::Sandbox),
            _ => Err(ValidationError::new(
                "environment",
                "must be live or sandbox",
            )),
        }
    }
}

/// A party trusted to attest payments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attestor {
    /// Identity of the attestor.
    pub id: AttestorId,
    /// Display name, e.g. "Stripe gateway".
    pub name: String,
    /// Which sales it may settle.
    pub environment: Environment,
    /// Revoked attestors are no longer believed.
    pub revoked: bool,
    /// When the attestor was registered.
    pub created_at: Timestamp,
}

/// A public key an attestor signs with. Attestors may hold several to rotate keys.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttestorKey {
    /// The attestor that owns the key.
    pub attestor_id: AttestorId,
    /// The attestor's own name for the key, sent with every signature.
    pub key_id: String,
    /// Ed25519 public key.
    pub public_key: [u8; 32],
    /// Revoked keys are no longer believed.
    pub revoked: bool,
    /// When the key was registered.
    pub created_at: Timestamp,
}

/// What became of the money an attestation reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaymentDisposition {
    /// The payment paid for issued tickets.
    Applied,
    /// The payment could not be used; the attestor must return it.
    RefundRequired,
    /// The attestor returned it.
    Refunded,
}

impl PaymentDisposition {
    /// The disposition's name as stored and serialized.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Applied => "applied",
            Self::RefundRequired => "refund_required",
            Self::Refunded => "refunded",
        }
    }
}

impl std::str::FromStr for PaymentDisposition {
    type Err = ValidationError;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        match name {
            "applied" => Ok(Self::Applied),
            "refund_required" => Ok(Self::RefundRequired),
            "refunded" => Ok(Self::Refunded),
            _ => Err(ValidationError::new(
                "disposition",
                "is not a known payment disposition",
            )),
        }
    }
}

/// An attestor's signed statement that a reservation was paid, as recorded by Kippu.
///
/// `(attestor_id, attestation_id)` is unique: an attestor may deliver the same attestation any
/// number of times and it is recorded — and acted on — once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaymentAttestation {
    /// Who attests.
    pub attestor_id: AttestorId,
    /// The attestor's own reference for the payment (e.g. a payment intent id).
    pub attestation_id: String,
    /// The reservation paid for.
    pub reservation_id: ReservationId,
    /// What was paid. Must equal the reservation's total exactly.
    pub amount: Money,
    /// When the attestor says the payment happened.
    pub occurred_at: Timestamp,
    /// When Kippu recorded it.
    pub received_at: Timestamp,
    /// What became of the money.
    pub disposition: PaymentDisposition,
}

/// Validates an attestor's reference for a payment: 1–128 visible ASCII characters.
pub fn validate_attestation_id(attestation_id: &str) -> Result<(), ValidationError> {
    if (1..=128).contains(&attestation_id.len())
        && attestation_id.bytes().all(|b| b.is_ascii_graphic())
    {
        Ok(())
    } else {
        Err(ValidationError::new(
            "attestation_id",
            "must be 1-128 visible ASCII characters",
        ))
    }
}
