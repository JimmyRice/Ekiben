use async_trait::async_trait;
use kippu_store::{
    AuditEntry, AuditRecord, HousekeepingStore, IdempotencyRecord, Insertion, PageRequest,
    StoreResult,
};

use crate::convert::{instant, micros};
use crate::{SqliteStore, error};

#[async_trait]
impl HousekeepingStore for SqliteStore {
    async fn idempotency_record(
        &self,
        scope: &str,
        key: &str,
    ) -> StoreResult<Option<IdempotencyRecord>> {
        let row = sqlx::query_as::<_, (String, u16, Vec<u8>, i64)>(
            "SELECT fingerprint, status, body, created_at FROM idempotency_records
             WHERE scope = ?1 AND key = ?2",
        )
        .bind(scope)
        .bind(key)
        .fetch_optional(&self.writer)
        .await
        .map_err(error)?;
        Ok(row.map(
            |(fingerprint, status, body, created_at)| IdempotencyRecord {
                fingerprint,
                status,
                body,
                created_at: instant(created_at),
            },
        ))
    }

    async fn save_idempotency_record(
        &self,
        scope: &str,
        key: &str,
        record: &IdempotencyRecord,
    ) -> StoreResult<Insertion<IdempotencyRecord>> {
        let inserted = sqlx::query(
            "INSERT INTO idempotency_records (scope, key, fingerprint, status, body, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT DO NOTHING",
        )
        .bind(scope)
        .bind(key)
        .bind(&record.fingerprint)
        .bind(record.status)
        .bind(record.body.as_slice())
        .bind(micros(record.created_at))
        .execute(&self.writer)
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
        sqlx::query("INSERT INTO audit_log (at, actor, action, target) VALUES (?1, ?2, ?3, ?4)")
            .bind(micros(entry.at))
            .bind(&entry.actor)
            .bind(&entry.action)
            .bind(&entry.target)
            .execute(&self.writer)
            .await
            .map_err(error)?;
        Ok(())
    }

    async fn audit_log(&self, page: PageRequest<i64>) -> StoreResult<Vec<AuditRecord>> {
        let rows = sqlx::query_as::<_, (i64, i64, String, String, String)>(
            "SELECT id, at, actor, action, target FROM audit_log
             WHERE ?1 IS NULL OR id < ?1 ORDER BY id DESC LIMIT ?2",
        )
        .bind(page.after)
        .bind(page.limit)
        .fetch_all(&self.reader)
        .await
        .map_err(error)?;
        Ok(rows
            .into_iter()
            .map(|(sequence, at, actor, action, target)| AuditRecord {
                sequence,
                entry: AuditEntry {
                    at: instant(at),
                    actor,
                    action,
                    target,
                },
            })
            .collect())
    }
}
