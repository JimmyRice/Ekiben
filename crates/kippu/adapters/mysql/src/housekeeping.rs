use async_trait::async_trait;
use kippu_store::{AuditEntry, HousekeepingStore, IdempotencyRecord, Insertion, StoreResult};

use crate::convert::{instant, micros};
use crate::{MySqlStore, error, is_duplicate};

#[async_trait]
impl HousekeepingStore for MySqlStore {
    async fn idempotency_record(
        &self,
        scope: &str,
        key: &str,
    ) -> StoreResult<Option<IdempotencyRecord>> {
        let row = sqlx::query_as::<_, (String, u16, Vec<u8>, i64)>(
            "SELECT fingerprint, status, body, created_at FROM idempotency_records
             WHERE scope = ? AND `key` = ?",
        )
        .bind(scope)
        .bind(key)
        .fetch_optional(&self.pool)
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
            "INSERT INTO idempotency_records (scope, `key`, fingerprint, status, body, created_at)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(scope)
        .bind(key)
        .bind(&record.fingerprint)
        .bind(record.status)
        .bind(record.body.as_slice())
        .bind(micros(record.created_at))
        .execute(&self.pool)
        .await;
        match inserted {
            Ok(_) => return Ok(Insertion::Inserted),
            Err(failure) if is_duplicate(&failure) => {}
            Err(failure) => return Err(error(failure)),
        }
        match self.idempotency_record(scope, key).await? {
            Some(existing) => Ok(Insertion::Existing(existing)),
            None => Ok(Insertion::Inserted),
        }
    }

    async fn append_audit(&self, entry: &AuditEntry) -> StoreResult<()> {
        sqlx::query("INSERT INTO audit_log (at, actor, action, target) VALUES (?, ?, ?, ?)")
            .bind(micros(entry.at))
            .bind(&entry.actor)
            .bind(&entry.action)
            .bind(&entry.target)
            .execute(&self.pool)
            .await
            .map_err(error)?;
        Ok(())
    }

    async fn audit_log(&self, limit: u32) -> StoreResult<Vec<AuditEntry>> {
        let rows = sqlx::query_as::<_, (i64, String, String, String)>(
            "SELECT at, actor, action, target FROM audit_log ORDER BY id DESC LIMIT ?",
        )
        .bind(limit)
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
