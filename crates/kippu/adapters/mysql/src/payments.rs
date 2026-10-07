use async_trait::async_trait;
use kippu_domain::payment::{Attestor, AttestorKey, PaymentAttestation, PaymentDisposition};
use kippu_domain::{AttestorId, ReservationId};
use kippu_store::{Insertion, PaymentStore, PaymentsTx, StoreResult};

use crate::convert::{AttestationRow, AttestorKeyRow, AttestorRow, all, micros, optional};
use crate::tx::MySqlTx;
use crate::{MySqlStore, error, is_duplicate, unique};

const ATTESTATION_COLUMNS: &str = "SELECT attestor_id, attestation_id, reservation_id, amount_minor, \
     currency, occurred_at, received_at, disposition FROM payment_attestations";

#[async_trait]
impl PaymentStore for MySqlStore {
    async fn insert_attestor(&self, attestor: &Attestor) -> StoreResult<()> {
        sqlx::query(
            "INSERT INTO attestors (id, name, environment, revoked, created_at)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(attestor.id.as_uuid())
        .bind(&attestor.name)
        .bind(attestor.environment.as_str())
        .bind(attestor.revoked)
        .bind(micros(attestor.created_at))
        .execute(&self.pool)
        .await
        .map_err(unique("id"))?;
        Ok(())
    }

    async fn attestor(&self, id: AttestorId) -> StoreResult<Option<Attestor>> {
        let row = sqlx::query_as::<_, AttestorRow>(
            "SELECT id, name, environment, revoked, created_at FROM attestors WHERE id = ?",
        )
        .bind(id.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(error)?;
        optional(row)
    }

    async fn list_attestors(&self) -> StoreResult<Vec<Attestor>> {
        let rows = sqlx::query_as::<_, AttestorRow>(
            "SELECT id, name, environment, revoked, created_at FROM attestors ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(error)?;
        all(rows)
    }

    async fn set_attestor_revoked(&self, id: AttestorId, revoked: bool) -> StoreResult<bool> {
        let result = sqlx::query("UPDATE attestors SET revoked = ? WHERE id = ?")
            .bind(revoked)
            .bind(id.as_uuid())
            .execute(&self.pool)
            .await
            .map_err(error)?;
        Ok(result.rows_affected() == 1)
    }

    async fn insert_attestor_key(&self, key: &AttestorKey) -> StoreResult<()> {
        sqlx::query(
            "INSERT INTO attestor_keys (attestor_id, key_id, public_key, revoked, created_at)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(key.attestor_id.as_uuid())
        .bind(&key.key_id)
        .bind(key.public_key.as_slice())
        .bind(key.revoked)
        .bind(micros(key.created_at))
        .execute(&self.pool)
        .await
        .map_err(unique("key_id"))?;
        Ok(())
    }

    async fn attestor_keys(&self, attestor: AttestorId) -> StoreResult<Vec<AttestorKey>> {
        let rows = sqlx::query_as::<_, AttestorKeyRow>(
            "SELECT attestor_id, key_id, public_key, revoked, created_at FROM attestor_keys
             WHERE attestor_id = ? ORDER BY key_id",
        )
        .bind(attestor.as_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(error)?;
        all(rows)
    }

    async fn attestor_key(
        &self,
        attestor: AttestorId,
        key_id: &str,
    ) -> StoreResult<Option<AttestorKey>> {
        let row = sqlx::query_as::<_, AttestorKeyRow>(
            "SELECT attestor_id, key_id, public_key, revoked, created_at FROM attestor_keys
             WHERE attestor_id = ? AND key_id = ?",
        )
        .bind(attestor.as_uuid())
        .bind(key_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(error)?;
        optional(row)
    }

    async fn set_attestor_key_revoked(
        &self,
        attestor: AttestorId,
        key_id: &str,
        revoked: bool,
    ) -> StoreResult<bool> {
        let result = sqlx::query(
            "UPDATE attestor_keys SET revoked = ? WHERE attestor_id = ? AND key_id = ?",
        )
        .bind(revoked)
        .bind(attestor.as_uuid())
        .bind(key_id)
        .execute(&self.pool)
        .await
        .map_err(error)?;
        Ok(result.rows_affected() == 1)
    }

    async fn attestation(
        &self,
        attestor: AttestorId,
        attestation_id: &str,
    ) -> StoreResult<Option<PaymentAttestation>> {
        let row = sqlx::query_as::<_, AttestationRow>(sqlx::AssertSqlSafe(format!(
            "{ATTESTATION_COLUMNS} WHERE attestor_id = ? AND attestation_id = ?"
        )))
        .bind(attestor.as_uuid())
        .bind(attestation_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(error)?;
        optional(row)
    }

    async fn attestations_for_reservation(
        &self,
        reservation: ReservationId,
    ) -> StoreResult<Vec<PaymentAttestation>> {
        let rows = sqlx::query_as::<_, AttestationRow>(sqlx::AssertSqlSafe(format!(
            "{ATTESTATION_COLUMNS} WHERE reservation_id = ? ORDER BY received_at"
        )))
        .bind(reservation.as_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(error)?;
        all(rows)
    }
}

#[async_trait]
impl PaymentsTx for MySqlTx {
    async fn insert_attestation(
        &mut self,
        attestation: &PaymentAttestation,
    ) -> StoreResult<Insertion<PaymentAttestation>> {
        let inserted = sqlx::query(
            "INSERT INTO payment_attestations (attestor_id, attestation_id, reservation_id,
                                               amount_minor, currency, occurred_at, received_at,
                                               disposition)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(attestation.attestor_id.as_uuid())
        .bind(&attestation.attestation_id)
        .bind(attestation.reservation_id.as_uuid())
        .bind(attestation.amount.amount_minor)
        .bind(attestation.amount.currency.as_str())
        .bind(micros(attestation.occurred_at))
        .bind(micros(attestation.received_at))
        .bind(attestation.disposition.as_str())
        .execute(&mut *self.conn)
        .await;
        match inserted {
            Ok(_) => return Ok(Insertion::Inserted),
            Err(failure) if is_duplicate(&failure) => {}
            Err(failure) => return Err(error(failure)),
        }
        let existing = sqlx::query_as::<_, AttestationRow>(sqlx::AssertSqlSafe(format!(
            "{ATTESTATION_COLUMNS} WHERE attestor_id = ? AND attestation_id = ?"
        )))
        .bind(attestation.attestor_id.as_uuid())
        .bind(&attestation.attestation_id)
        .fetch_one(&mut *self.conn)
        .await
        .map_err(error)?;
        Ok(Insertion::Existing(existing.try_into()?))
    }

    async fn set_disposition(
        &mut self,
        attestor: AttestorId,
        attestation_id: &str,
        disposition: PaymentDisposition,
    ) -> StoreResult<()> {
        sqlx::query(
            "UPDATE payment_attestations SET disposition = ?
             WHERE attestor_id = ? AND attestation_id = ?",
        )
        .bind(disposition.as_str())
        .bind(attestor.as_uuid())
        .bind(attestation_id)
        .execute(&mut *self.conn)
        .await
        .map_err(error)?;
        Ok(())
    }
}
