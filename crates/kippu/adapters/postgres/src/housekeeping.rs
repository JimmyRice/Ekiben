use async_trait::async_trait;
use kippu_store::{
    AuditEntry, HousekeepingStore, IdempotencyRecord, Insertion, StoreError, StoreResult,
};
use time::OffsetDateTime;

use crate::convert::{at, instant};
use crate::{PostgresStore, error};

#[async_trait]
impl HousekeepingStore for PostgresStore {
    async fn idempotency_record(
        &self,
        scope: &str,
        key: &str,
    ) -> StoreResult<Option<IdempotencyRecord>> {
        let row = sqlx::query_as::<_, (String, i32, Vec<u8>, OffsetDateTime)>(
            "SELECT fingerprint, status, body, created_at FROM idempotency_records
             WHERE scope = $1 AND key = $2",
        )
        .bind(scope)
        .bind(key)
        .fetch_optional(&self.pool)
        .await
        .map_err(error)?;
        row.map(|(fingerprint, status, body, created_at)| {
            Ok(IdempotencyRecord {
                fingerprint,
                status: u16::try_from(status).map_err(StoreError::backend)?,
                body,
                created_at: instant(created_at),
            })
        })
        .transpose()
    }

    async fn save_idempotency_record(
        &self,
        scope: &str,
        key: &str,
        record: &IdempotencyRecord,
    ) -> StoreResult<Insertion<IdempotencyRecord>> {
        let inserted = sqlx::query(
            "INSERT INTO idempotency_records (scope, key, fingerprint, status, body, created_at)
             VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT DO NOTHING",
        )
        .bind(scope)
        .bind(key)
        .bind(&record.fingerprint)
        .bind(i32::from(record.status))
        .bind(record.body.as_slice())
        .bind(at(record.created_at))
        .execute(&self.pool)
        .await
        .map_err(error)?;
        if inserted.rows_affected() == 1 {
            return Ok(Insertion::Inserted);
        }
        match self.idempotency_record(scope, key).await? {
            Some(existing) => Ok(Insertion::Existing(existing)),
            None => Ok(Insertion::Inserted),
        }
    }

    async fn append_audit(&self, entry: &AuditEntry) -> StoreResult<()> {
        sqlx::query("INSERT INTO audit_log (at, actor, action, target) VALUES ($1, $2, $3, $4)")
            .bind(at(entry.at))
            .bind(&entry.actor)
            .bind(&entry.action)
            .bind(&entry.target)
            .execute(&self.pool)
            .await
            .map_err(error)?;
        Ok(())
    }

    async fn audit_log(&self, limit: u32) -> StoreResult<Vec<AuditEntry>> {
        let rows = sqlx::query_as::<_, (OffsetDateTime, String, String, String)>(
            "SELECT at, actor, action, target FROM audit_log ORDER BY id DESC LIMIT $1",
        )
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(error)?;
        Ok(rows
            .into_iter()
            .map(|(at, actor, action, target)| AuditEntry {
                at: instant(at),
                actor,
                action,
                target,
            })
            .collect())
    }
}
