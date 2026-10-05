use async_trait::async_trait;
use kippu_domain::Timestamp;
use kippu_domain::outbox::IntegrationEvent;
use kippu_store::{OutboxRecord, OutboxStore, OutboxTx, StoreResult};
use sqlx::types::Json;
use time::OffsetDateTime;

use crate::convert::{at, instant};
use crate::tx::PostgresTx;
use crate::{PostgresStore, error};

/// The advisory lock key that serializes outbox appends ("kippuobx").
const OUTBOX_LOCK: i64 = 0x6b69_7070_756f_6278;

#[async_trait]
impl OutboxStore for PostgresStore {
    async fn outbox_after(&self, after: i64, limit: u32) -> StoreResult<Vec<OutboxRecord>> {
        let rows = sqlx::query_as::<_, (i64, Json<IntegrationEvent>, OffsetDateTime)>(
            "SELECT sequence, payload, created_at FROM outbox WHERE sequence > $1
             ORDER BY sequence LIMIT $2",
        )
        .bind(after)
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(error)?;
        Ok(rows
            .into_iter()
            .map(|(sequence, Json(event), created_at)| OutboxRecord {
                sequence,
                event,
                created_at: instant(created_at),
            })
            .collect())
    }
}

#[async_trait]
impl OutboxTx for PostgresTx {
    async fn append_event(&mut self, event: &IntegrationEvent, now: Timestamp) -> StoreResult<()> {
        // Held until commit: appends are serialized, so sequence numbers follow commit order.
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(OUTBOX_LOCK)
            .execute(&mut *self.conn)
            .await
            .map_err(error)?;
        sqlx::query("INSERT INTO outbox (topic, payload, created_at) VALUES ($1, $2, $3)")
            .bind(event.topic())
            .bind(Json(event))
            .bind(at(now))
            .execute(&mut *self.conn)
            .await
            .map_err(error)?;
        Ok(())
    }
}
