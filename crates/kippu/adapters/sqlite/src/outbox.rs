use async_trait::async_trait;
use kippu_domain::Timestamp;
use kippu_domain::outbox::IntegrationEvent;
use kippu_store::{OutboxRecord, OutboxStore, OutboxTx, StoreError, StoreResult};

use crate::convert::{instant, json, micros};
use crate::tx::SqliteTx;
use crate::{SqliteStore, error};

#[async_trait]
impl OutboxStore for SqliteStore {
    async fn outbox_after(&self, after: i64, limit: u32) -> StoreResult<Vec<OutboxRecord>> {
        let rows = sqlx::query_as::<_, (i64, String, i64)>(
            "SELECT sequence, payload, created_at FROM outbox WHERE sequence > ?1
             ORDER BY sequence LIMIT ?2",
        )
        .bind(after)
        .bind(limit)
        .fetch_all(&self.reader)
        .await
        .map_err(error)?;
        rows.into_iter()
            .map(|(sequence, payload, created_at)| {
                Ok(OutboxRecord {
                    sequence,
                    event: serde_json::from_str(&payload).map_err(StoreError::backend)?,
                    created_at: instant(created_at),
                })
            })
            .collect()
    }

    async fn latest_sequence(&self) -> StoreResult<i64> {
        sqlx::query_scalar::<_, i64>("SELECT COALESCE(MAX(sequence), 0) FROM outbox")
            .fetch_one(&self.reader)
            .await
            .map_err(error)
    }
}

#[async_trait]
impl OutboxTx for SqliteTx {
    async fn append_event(&mut self, event: &IntegrationEvent, now: Timestamp) -> StoreResult<()> {
        sqlx::query("INSERT INTO outbox (topic, payload, created_at) VALUES (?1, ?2, ?3)")
            .bind(event.topic())
            .bind(json(event)?)
            .bind(micros(now))
            .execute(&mut *self.conn)
            .await
            .map_err(error)?;
        Ok(())
    }
}
