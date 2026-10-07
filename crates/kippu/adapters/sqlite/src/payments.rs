use async_trait::async_trait;
use kippu_domain::payment::{Attestor, AttestorKey, PaymentAttestation, PaymentDisposition};
use kippu_domain::refund::Refund;
use kippu_domain::{AttestorId, RefundId, ReservationId, Timestamp};
use kippu_store::{Insertion, PaymentStore, PaymentsTx, StoreResult};

use crate::convert::{
    AttestationRow, AttestorKeyRow, AttestorRow, RefundRow, all, json, micros, optional,
};
use crate::tx::SqliteTx;
use crate::{SqliteStore, error, unique};

const REFUND_COLUMNS: &str = "SELECT id, reservation_id, account_id, event_id, attestor_id, \
     attestation_id, ticket_ids, amount_minor, currency, reason, status, created_at, \
     completed_at FROM refunds";

const ATTESTATION_COLUMNS: &str = "SELECT attestor_id, attestation_id, reservation_id, amount_minor, \
     currency, occurred_at, received_at, disposition FROM payment_attestations";

#[async_trait]
impl PaymentStore for SqliteStore {
    async fn insert_attestor(&self, attestor: &Attestor) -> StoreResult<()> {
        sqlx::query(
            "INSERT INTO attestors (id, name, environment, revoked, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .bind(attestor.id.as_uuid())
        .bind(&attestor.name)
        .bind(attestor.environment.as_str())
        .bind(attestor.revoked)
        .bind(micros(attestor.created_at))
        .execute(&self.writer)
        .await
        .map_err(unique("id"))?;
        Ok(())
    }

    async fn attestor(&self, id: AttestorId) -> StoreResult<Option<Attestor>> {
        let row = sqlx::query_as::<_, AttestorRow>(
            "SELECT id, name, environment, revoked, created_at FROM attestors WHERE id = ?1",
        )
        .bind(id.as_uuid())
        .fetch_optional(&self.reader)
        .await
        .map_err(error)?;
        optional(row)
    }

    async fn list_attestors(&self) -> StoreResult<Vec<Attestor>> {
        let rows = sqlx::query_as::<_, AttestorRow>(
            "SELECT id, name, environment, revoked, created_at FROM attestors ORDER BY id",
        )
        .fetch_all(&self.reader)
        .await
        .map_err(error)?;
        all(rows)
    }

    async fn set_attestor_revoked(&self, id: AttestorId, revoked: bool) -> StoreResult<bool> {
        let result = sqlx::query("UPDATE attestors SET revoked = ?2 WHERE id = ?1")
            .bind(id.as_uuid())
            .bind(revoked)
            .execute(&self.writer)
            .await
            .map_err(error)?;
        Ok(result.rows_affected() == 1)
    }

    async fn insert_attestor_key(&self, key: &AttestorKey) -> StoreResult<()> {
        sqlx::query(
            "INSERT INTO attestor_keys (attestor_id, key_id, public_key, revoked, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .bind(key.attestor_id.as_uuid())
        .bind(&key.key_id)
        .bind(key.public_key.as_slice())
        .bind(key.revoked)
        .bind(micros(key.created_at))
        .execute(&self.writer)
        .await
        .map_err(unique("key_id"))?;
        Ok(())
    }

    async fn attestor_keys(&self, attestor: AttestorId) -> StoreResult<Vec<AttestorKey>> {
        let rows = sqlx::query_as::<_, AttestorKeyRow>(
            "SELECT attestor_id, key_id, public_key, revoked, created_at FROM attestor_keys
             WHERE attestor_id = ?1 ORDER BY key_id",
        )
        .bind(attestor.as_uuid())
        .fetch_all(&self.reader)
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
             WHERE attestor_id = ?1 AND key_id = ?2",
        )
        .bind(attestor.as_uuid())
        .bind(key_id)
        .fetch_optional(&self.reader)
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
            "UPDATE attestor_keys SET revoked = ?3 WHERE attestor_id = ?1 AND key_id = ?2",
        )
        .bind(attestor.as_uuid())
        .bind(key_id)
        .bind(revoked)
        .execute(&self.writer)
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
            "{ATTESTATION_COLUMNS} WHERE attestor_id = ?1 AND attestation_id = ?2"
        )))
        .bind(attestor.as_uuid())
        .bind(attestation_id)
        .fetch_optional(&self.reader)
        .await
        .map_err(error)?;
        optional(row)
    }

    async fn attestations_for_reservation(
        &self,
        reservation: ReservationId,
    ) -> StoreResult<Vec<PaymentAttestation>> {
        let rows = sqlx::query_as::<_, AttestationRow>(sqlx::AssertSqlSafe(format!(
            "{ATTESTATION_COLUMNS} WHERE reservation_id = ?1 ORDER BY received_at"
        )))
        .bind(reservation.as_uuid())
        .fetch_all(&self.reader)
        .await
        .map_err(error)?;
        all(rows)
    }

    async fn refund(&self, id: RefundId) -> StoreResult<Option<Refund>> {
        let row = sqlx::query_as::<_, RefundRow>(sqlx::AssertSqlSafe(format!(
            "{REFUND_COLUMNS} WHERE id = ?1"
        )))
        .bind(id.as_uuid())
        .fetch_optional(&self.reader)
        .await
        .map_err(error)?;
        optional(row)
    }

    async fn refunds_for_reservation(
        &self,
        reservation: ReservationId,
    ) -> StoreResult<Vec<Refund>> {
        let rows = sqlx::query_as::<_, RefundRow>(sqlx::AssertSqlSafe(format!(
            "{REFUND_COLUMNS} WHERE reservation_id = ?1 ORDER BY created_at, id"
        )))
        .bind(reservation.as_uuid())
        .fetch_all(&self.reader)
        .await
        .map_err(error)?;
        all(rows)
    }
}

#[async_trait]
impl PaymentsTx for SqliteTx {
    async fn insert_attestation(
        &mut self,
        attestation: &PaymentAttestation,
    ) -> StoreResult<Insertion<PaymentAttestation>> {
        let inserted = sqlx::query(
            "INSERT INTO payment_attestations (attestor_id, attestation_id, reservation_id,
                                               amount_minor, currency, occurred_at, received_at,
                                               disposition)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT (attestor_id, attestation_id) DO NOTHING",
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
        .await
        .map_err(error)?;
        if inserted.rows_affected() == 1 {
            return Ok(Insertion::Inserted);
        }
        let existing = sqlx::query_as::<_, AttestationRow>(sqlx::AssertSqlSafe(format!(
            "{ATTESTATION_COLUMNS} WHERE attestor_id = ?1 AND attestation_id = ?2"
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
            "UPDATE payment_attestations SET disposition = ?3
             WHERE attestor_id = ?1 AND attestation_id = ?2",
        )
        .bind(attestor.as_uuid())
        .bind(attestation_id)
        .bind(disposition.as_str())
        .execute(&mut *self.conn)
        .await
        .map_err(error)?;
        Ok(())
    }

    async fn insert_refund(&mut self, refund: &Refund) -> StoreResult<Insertion<Refund>> {
        let inserted = sqlx::query(
            "INSERT INTO refunds (id, reservation_id, account_id, event_id, attestor_id,
                                  attestation_id, ticket_ids, amount_minor, currency, reason,
                                  status, created_at, completed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
             ON CONFLICT (id) DO NOTHING",
        )
        .bind(refund.id.as_uuid())
        .bind(refund.reservation_id.as_uuid())
        .bind(refund.account_id.as_uuid())
        .bind(refund.event_id.as_uuid())
        .bind(refund.attestor_id.as_uuid())
        .bind(&refund.attestation_id)
        .bind(json(&refund.ticket_ids)?)
        .bind(refund.amount.amount_minor)
        .bind(refund.amount.currency.as_str())
        .bind(refund.reason.as_str())
        .bind(refund.status.as_str())
        .bind(micros(refund.created_at))
        .bind(refund.completed_at.map(micros))
        .execute(&mut *self.conn)
        .await
        .map_err(error)?;
        if inserted.rows_affected() == 1 {
            return Ok(Insertion::Inserted);
        }
        let existing = sqlx::query_as::<_, RefundRow>(sqlx::AssertSqlSafe(format!(
            "{REFUND_COLUMNS} WHERE id = ?1"
        )))
        .bind(refund.id.as_uuid())
        .fetch_one(&mut *self.conn)
        .await
        .map_err(error)?;
        Ok(Insertion::Existing(existing.try_into()?))
    }

    async fn complete_refund(&mut self, id: RefundId, at: Timestamp) -> StoreResult<bool> {
        let completed = sqlx::query(
            "UPDATE refunds SET status = 'completed', completed_at = ?2
             WHERE id = ?1 AND status = 'pending'",
        )
        .bind(id.as_uuid())
        .bind(micros(at))
        .execute(&mut *self.conn)
        .await
        .map_err(error)?;
        Ok(completed.rows_affected() == 1)
    }
}
