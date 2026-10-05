use async_trait::async_trait;
use kippu_domain::Timestamp;
use kippu_domain::outbox::IntegrationEvent;

use crate::StoreResult;

/// An integration event as stored in the outbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxRecord {
    /// Strictly increasing position in the outbox. Consumers remember the last one they saw.
    pub sequence: i64,
    /// What happened.
    pub event: IntegrationEvent,
    /// When it was recorded.
    pub created_at: Timestamp,
}

/// Reading the outbox.
#[async_trait]
pub trait OutboxStore {
    /// Up to `limit` events with a sequence number above `after`, in order.
    async fn outbox_after(&self, after: i64, limit: u32) -> StoreResult<Vec<OutboxRecord>>;
}

/// Writing the outbox inside a transaction.
#[async_trait]
pub trait OutboxTx: Send {
    /// Appends an event. It becomes visible exactly when the transaction commits.
    async fn append_event(&mut self, event: &IntegrationEvent, now: Timestamp) -> StoreResult<()>;
}
