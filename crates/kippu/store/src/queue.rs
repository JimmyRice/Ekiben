//! Messaging ports: an optional queue in front of the database, and an optional bus for
//! integration events. Without them, the database is the queue (purchase requests are
//! inserted directly) and events are read from the outbox feed.

use async_trait::async_trait;
use kippu_domain::purchase::PurchaseRequest;

use crate::{OutboxRecord, StoreResult};

/// A purchase request taken from the inbox, to acknowledge once it is persisted.
#[async_trait]
pub trait InboxDelivery: Send {
    /// The request.
    fn request(&self) -> &PurchaseRequest;

    /// Removes the request from the inbox. Until then it is redelivered after a while.
    async fn ack(self: Box<Self>) -> StoreResult<()>;
}

/// A durable queue purchase requests pass through before reaching the database, so that
/// bursts are absorbed by the queue instead of the database.
///
/// **Contract:** `enqueue` returns only once the request is durably stored; enqueueing the
/// same request id again (a retry) delivers it at most once more within the queue's
/// deduplication window, and persisting is idempotent anyway. Every enqueued request is
/// delivered by `receive` until acknowledged.
#[async_trait]
pub trait PurchaseInbox: Send + Sync {
    /// Durably accepts a request.
    async fn enqueue(&self, request: &PurchaseRequest) -> StoreResult<()>;

    /// Takes up to `limit` requests, waiting only briefly when there are none.
    async fn receive(&self, limit: usize) -> StoreResult<Vec<Box<dyn InboxDelivery>>>;
}

/// Where integration events are published, in outbox order.
///
/// **Contract:** publishing the same sequence again is a no-op within the bus's
/// deduplication window, so several relays may run at once; `last_published` is the
/// highest sequence published, or 0.
#[async_trait]
pub trait EventBus: Send + Sync {
    /// The highest outbox sequence on the bus, or 0.
    async fn last_published(&self) -> StoreResult<i64>;

    /// Publishes one event; returns once the bus has stored it.
    async fn publish(&self, record: &OutboxRecord) -> StoreResult<()>;
}
