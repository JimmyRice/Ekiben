use async_trait::async_trait;
use kippu_domain::payment::{Attestor, AttestorKey, PaymentAttestation, PaymentDisposition};
use kippu_domain::{AttestorId, ReservationId};

use crate::{Insertion, StoreResult};

/// Attestors, their keys and recorded attestations.
#[async_trait]
pub trait PaymentStore {
    /// Registers an attestor.
    async fn insert_attestor(&self, attestor: &Attestor) -> StoreResult<()>;
    /// Looks an attestor up by id.
    async fn attestor(&self, id: AttestorId) -> StoreResult<Option<Attestor>>;
    /// Every attestor, built-in ones included.
    async fn list_attestors(&self) -> StoreResult<Vec<Attestor>>;
    /// Revokes or restores an attestor. Returns whether it exists.
    async fn set_attestor_revoked(&self, id: AttestorId, revoked: bool) -> StoreResult<bool>;

    /// Registers a signing key. Fails with `Conflict("key_id")` if the attestor already has a
    /// key with that id.
    async fn insert_attestor_key(&self, key: &AttestorKey) -> StoreResult<()>;
    /// An attestor's keys.
    async fn attestor_keys(&self, attestor: AttestorId) -> StoreResult<Vec<AttestorKey>>;
    /// Looks a key up by the attestor's name for it.
    async fn attestor_key(
        &self,
        attestor: AttestorId,
        key_id: &str,
    ) -> StoreResult<Option<AttestorKey>>;
    /// Revokes or restores a key. Returns whether it exists.
    async fn set_attestor_key_revoked(
        &self,
        attestor: AttestorId,
        key_id: &str,
        revoked: bool,
    ) -> StoreResult<bool>;

    /// Looks an attestation up by the attestor's reference.
    async fn attestation(
        &self,
        attestor: AttestorId,
        attestation_id: &str,
    ) -> StoreResult<Option<PaymentAttestation>>;
    /// Every attestation recorded for a reservation.
    async fn attestations_for_reservation(
        &self,
        reservation: ReservationId,
    ) -> StoreResult<Vec<PaymentAttestation>>;
}

/// Attestations inside a transaction.
#[async_trait]
pub trait PaymentsTx: Send {
    /// Records an attestation.
    ///
    /// **Contract:** `(attestor_id, attestation_id)` is unique. Recording the same attestation
    /// again writes nothing and returns the stored one, so a delivery repeated N times is
    /// acted on once.
    async fn insert_attestation(
        &mut self,
        attestation: &PaymentAttestation,
    ) -> StoreResult<Insertion<PaymentAttestation>>;

    /// Changes what became of an attested payment.
    async fn set_disposition(
        &mut self,
        attestor: AttestorId,
        attestation_id: &str,
        disposition: PaymentDisposition,
    ) -> StoreResult<()>;
}
