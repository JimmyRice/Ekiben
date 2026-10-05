use kippu_domain::outbox::IntegrationEvent;
use kippu_domain::payment::{Attestor, Environment, PaymentDisposition};
use kippu_domain::reservation::Reservation;
use kippu_domain::{Money, ReservationId, TicketId, Timestamp};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

/// Register an attestor and its first keys.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateAttestorRequest {
    /// Display name, e.g. "Stripe gateway".
    pub name: String,
    /// `live` or `sandbox`. Sandbox attestors can only settle sandbox sales.
    pub environment: Environment,
    /// Signing keys.
    #[serde(default)]
    pub keys: Vec<AttestorKeyRequest>,
}

/// Register a signing key for an attestor.
#[derive(Debug, Deserialize, ToSchema)]
pub struct AttestorKeyRequest {
    /// The attestor's name for the key, sent in `Kippu-Signature`.
    pub key_id: String,
    /// Ed25519 public key, base64 or SPKI PEM.
    pub public_key: String,
}

/// Revoke or restore.
#[derive(Debug, Deserialize, ToSchema)]
pub struct RevokedRequest {
    /// `true` to stop believing it.
    pub revoked: bool,
}

/// An attestor and its keys.
#[derive(Debug, Serialize, ToSchema)]
pub struct AttestorView {
    /// The attestor.
    #[serde(flatten)]
    pub attestor: Attestor,
    /// Its signing keys.
    pub keys: Vec<AttestorKeyView>,
}

/// A registered signing key.
#[derive(Debug, Serialize, ToSchema)]
pub struct AttestorKeyView {
    /// The attestor's name for the key.
    pub key_id: String,
    /// Ed25519 public key, base64.
    pub public_key: String,
    /// Whether it is no longer believed.
    pub revoked: bool,
}

/// What happened to the money.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PaymentOutcome {
    /// The buyer paid.
    Paid,
    /// The attestor returned a payment Kippu reported as `refund.required`.
    Refunded,
}

/// An attestor's statement about a payment. Send it signed (see `Kippu-Signature`).
#[derive(Debug, Deserialize, ToSchema)]
pub struct AttestationRequest {
    /// The attestor's own reference for the payment, unique per attestor. Retries reuse it.
    pub attestation_id: String,
    /// What happened.
    pub outcome: PaymentOutcome,
    /// The reservation paid for (required when `outcome` is `paid`).
    pub reservation_id: Option<ReservationId>,
    /// The amount paid; must equal the reservation total (required when `outcome` is `paid`).
    pub amount: Option<Money>,
    /// When the payment happened (defaults to now).
    pub occurred_at: Option<Timestamp>,
}

/// Record a payment taken in person.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ManualPaymentRequest {
    /// Your reference for the payment, e.g. a receipt number. Unique.
    pub attestation_id: String,
    /// The amount received; must equal the reservation total.
    pub amount: Money,
}

/// The result of settling a payment.
#[derive(Debug, Serialize, ToSchema)]
pub struct SettlementView {
    /// What became of the money: `applied` (tickets issued), `refund_required` or `refunded`.
    pub disposition: PaymentDisposition,
    /// The reservation afterwards.
    pub reservation: Reservation,
    /// Tickets issued for the reservation.
    pub ticket_ids: Vec<TicketId>,
    /// `true` if this attestation had already been recorded and nothing changed.
    pub replayed: bool,
}

/// Where to continue reading an event feed.
#[derive(Debug, Deserialize, IntoParams)]
pub struct FeedQuery {
    /// Return events after this sequence number (default 0).
    pub after: Option<i64>,
    /// At most this many events (default 100, at most 500).
    pub limit: Option<u32>,
}

/// An integration event from the outbox.
#[derive(Debug, Serialize, ToSchema)]
pub struct FeedEvent {
    /// Position in the feed; pass the last one you processed as `after`.
    pub sequence: i64,
    /// When it happened.
    pub created_at: Timestamp,
    /// What happened.
    pub event: IntegrationEvent,
}
