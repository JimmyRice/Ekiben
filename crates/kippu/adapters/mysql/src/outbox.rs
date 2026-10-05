use async_trait::async_trait;
use kippu_domain::Timestamp;
use kippu_domain::outbox::IntegrationEvent;
use kippu_store::{OutboxRecord, OutboxStore, OutboxTx, StoreError, StoreResult};

use crate::convert::{instant, json, micros};
use crate::tx::MySqlTx;
use crate::{MySqlStore, error};

#[async_trait]
impl OutboxStore for MySqlStore {
    async fn outbox_after(&self, after: i64, limit: u32) -> StoreResult<Vec<OutboxRecord>> {
        let rows = sqlx::query_as::<_, (i64, String, i64)>(
            "SELECT sequence, payload, created_at FROM outbox WHERE sequence > ?
             ORDER BY sequence LIMIT ?",
        )
        .bind(after)
        .bind(limit)
        .fetch_all(&self.pool)
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
}

#[async_trait]
impl OutboxTx for MySqlTx {
    async fn append_event(&mut self, event: &IntegrationEvent, now: Timestamp) -> StoreResult<()> {
        // Held until commit: appends are serialized, so sequence numbers follow commit order.
        sqlx::query("SELECT id FROM outbox_lock WHERE id = 1 FOR UPDATE")
            .execute(&mut *self.conn)
            .await
            .map_err(error)?;
        sqlx::query("INSERT INTO outbox (topic, payload, created_at) VALUES (?, ?, ?)")
            .bind(event.topic())
            .bind(json(event)?)
            .bind(micros(now))
            .execute(&mut *self.conn)
            .await
            .map_err(error)?;
        Ok(())
    }
}
