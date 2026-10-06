//! The event bus: outbox events relayed to `<prefix>.events.<topic>`.

use async_nats::HeaderMap;
use async_trait::async_trait;
use kippu_store::{EventBus, OutboxRecord, StoreError, StoreResult};

use crate::queue::{NatsQueue, unavailable};

/// The header carrying an event's outbox sequence.
pub const SEQUENCE_HEADER: &str = "Kippu-Sequence";

#[async_trait]
impl EventBus for NatsQueue {
    async fn last_published(&self) -> StoreResult<i64> {
        kippu_telemetry::call("nats", "last published", self.last_sequence()).await
    }

    async fn publish(&self, record: &OutboxRecord) -> StoreResult<()> {
        self.relay(record).await
    }
}

impl NatsQueue {
    async fn last_sequence(&self) -> StoreResult<i64> {
        let mut events = self.events.clone();
        let last = events
            .info()
            .await
            .map_err(unavailable)?
            .state
            .last_sequence;
        if last == 0 {
            return Ok(0);
        }
        let message = events.get_raw_message(last).await.map_err(unavailable)?;
        message
            .headers
            .get(SEQUENCE_HEADER)
            .and_then(|value| value.as_str().parse().ok())
            .ok_or_else(|| StoreError::backend("the last event on the bus has no Kippu-Sequence"))
    }

    /// Publishes one outbox record to its topic; [`NatsQueue::publish`] traces the call.
    async fn relay(&self, record: &OutboxRecord) -> StoreResult<()> {
        let payload = serde_json::to_vec(&serde_json::json!({
            "sequence": record.sequence,
            "created_at": record.created_at,
            "event": record.event,
        }))
        .map_err(StoreError::backend)?;
        let mut headers = HeaderMap::new();
        headers.insert(SEQUENCE_HEADER, record.sequence.to_string().as_str());
        NatsQueue::publish(
            self,
            format!("{}.{}", self.events_prefix, record.event.topic()),
            format!("outbox-{}", record.sequence),
            headers,
            payload,
        )
        .await
    }
}
