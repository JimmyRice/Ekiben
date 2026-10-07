use kippu_domain::outbox::IntegrationEvent;
use kippu_domain::payment::{Attestor, Environment, PaymentDisposition};
use kippu_domain::refund::Refund;
use kippu_domain::reservation::Reservation;
use kippu_domain::{Money, RefundId, ReservationId, TicketId, Timestamp, ValidationError};
use kippu_store::OutboxRecord;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use super::service::{
    AttestorReport, AttestorWithKeys, IncomingPayment, ManualPayment, NewAttestor, NewAttestorKey,
    PaymentResult, RefundRequest,
};
use crate::keys::encode_key;

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
    /// The attestor returned money Kippu asked for with `refund.required`.
    Refunded,
    /// The money went back without Kippu asking (a chargeback, a refund made in the
    /// provider's dashboard); the tickets it paid for are revoked.
    Reversed,
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
    /// With `refunded`: the refund of issued tickets returned, as `refund.required` named it.
    /// Leave out when `refund.required` carried none.
    pub refund_id: Option<RefundId>,
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
    /// The refund of issued tickets the report confirmed (`refunded` with a `refund_id`) or
    /// caused (`reversed`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refund: Option<Refund>,
}

/// Refund tickets of a reservation.
#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RefundTicketsRequest {
    /// The tickets to refund. Absent or empty: every ticket of the reservation still valid.
    #[serde(default)]
    pub ticket_ids: Vec<TicketId>,
}

/// Where to continue reading an event feed.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
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

impl From<CreateAttestorRequest> for NewAttestor {
    fn from(request: CreateAttestorRequest) -> Self {
        Self {
            name: request.name,
            environment: request.environment,
            keys: request.keys.into_iter().map(NewAttestorKey::from).collect(),
        }
    }
}

impl From<AttestorKeyRequest> for NewAttestorKey {
    fn from(request: AttestorKeyRequest) -> Self {
        Self {
            key_id: request.key_id,
            public_key: request.public_key,
        }
    }
}

impl From<AttestorWithKeys> for AttestorView {
    fn from(attestor: AttestorWithKeys) -> Self {
        Self {
            attestor: attestor.attestor,
            keys: attestor
                .keys
                .into_iter()
                .map(|key| AttestorKeyView {
                    key_id: key.key_id,
                    public_key: encode_key(&key.public_key),
                    revoked: key.revoked,
                })
                .collect(),
        }
    }
}

impl TryFrom<AttestationRequest> for AttestorReport {
    type Error = ValidationError;

    fn try_from(request: AttestationRequest) -> Result<Self, ValidationError> {
        if request.refund_id.is_some() && request.outcome != PaymentOutcome::Refunded {
            return Err(ValidationError::new(
                "refund_id",
                "is only sent with the refunded outcome",
            ));
        }
        Ok(match request.outcome {
            PaymentOutcome::Paid => Self::Paid(IncomingPayment {
                attestation_id: request.attestation_id,
                reservation_id: request.reservation_id.ok_or(ValidationError::new(
                    "reservation_id",
                    "is required for a payment",
                ))?,
                amount: request
                    .amount
                    .ok_or(ValidationError::new("amount", "is required for a payment"))?,
                occurred_at: request.occurred_at,
            }),
            PaymentOutcome::Refunded => Self::Refunded {
                attestation_id: request.attestation_id,
                refund_id: request.refund_id,
            },
            PaymentOutcome::Reversed => Self::Reversed {
                attestation_id: request.attestation_id,
            },
        })
    }
}

impl From<ManualPaymentRequest> for ManualPayment {
    fn from(request: ManualPaymentRequest) -> Self {
        Self {
            attestation_id: request.attestation_id,
            amount: request.amount,
        }
    }
}

impl From<PaymentResult> for SettlementView {
    fn from(result: PaymentResult) -> Self {
        Self {
            disposition: result.disposition,
            reservation: result.reservation,
            ticket_ids: result.ticket_ids,
            replayed: result.replayed,
            refund: result.refund,
        }
    }
}

impl From<RefundTicketsRequest> for RefundRequest {
    fn from(request: RefundTicketsRequest) -> Self {
        Self {
            ticket_ids: request.ticket_ids,
        }
    }
}

impl FeedQuery {
    /// Where to start, and how many events to read at most.
    pub(crate) fn page(&self) -> (i64, u32) {
        (
            self.after.unwrap_or(0),
            self.limit.unwrap_or(100).clamp(1, 500),
        )
    }
}

impl From<OutboxRecord> for FeedEvent {
    fn from(record: OutboxRecord) -> Self {
        Self {
            sequence: record.sequence,
            created_at: record.created_at,
            event: record.event,
        }
    }
}
