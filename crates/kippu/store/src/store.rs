//! The store itself and its transactions: every port, put together.

use async_trait::async_trait;

use crate::{
    AccountStore, CatalogStore, HousekeepingStore, ImageStore, InventoryTx, OutboxStore, OutboxTx,
    PaymentStore, PaymentsTx, PurchaseStore, PurchasesTx, ReservationsTx, StoreResult, TicketStore,
    TicketsTx, WebhookStore,
};

/// A database behind Kippu.
///
/// Reads and single-record writes are available directly. Anything that must change several
/// records together goes through a transaction from [`Store::begin`].
#[async_trait]
pub trait Store:
    AccountStore
    + CatalogStore
    + PurchaseStore
    + PaymentStore
    + TicketStore
    + OutboxStore
    + HousekeepingStore
    + WebhookStore
    + ImageStore
    + Send
    + Sync
    + 'static
{
    /// What this backend can do, so callers can adapt (e.g. how many workers to run).
    fn capabilities(&self) -> StoreCapabilities;

    /// Applies the adapter's schema migrations. Safe to call on every start.
    async fn migrate(&self) -> StoreResult<()>;

    /// Checks that the database is reachable, for readiness probes.
    async fn ping(&self) -> StoreResult<()>;

    /// Starts a transaction.
    ///
    /// **Contract:** every write made through the transaction becomes visible atomically on
    /// [`StoreTx::commit`]. Dropping the transaction without committing discards all of them.
    async fn begin(&self) -> StoreResult<Box<dyn StoreTx>>;
}

/// A database transaction, split by concern so call sites read like the business process:
///
/// ```ignore
/// let mut tx = store.begin().await?;
/// if tx.inventory().try_hold(&holds).await? {
///     tx.reservations().insert_reservation(&reservation).await?;
/// }
/// tx.commit().await?;
/// ```
#[async_trait]
pub trait StoreTx: Send {
    /// Purchase requests.
    fn purchases(&mut self) -> &mut dyn PurchasesTx;
    /// Stock and per-account limits.
    fn inventory(&mut self) -> &mut dyn InventoryTx;
    /// Reservations.
    fn reservations(&mut self) -> &mut dyn ReservationsTx;
    /// Payment attestations.
    fn payments(&mut self) -> &mut dyn PaymentsTx;
    /// Issued tickets.
    fn tickets(&mut self) -> &mut dyn TicketsTx;
    /// Integration events, written atomically with the change they describe.
    fn outbox(&mut self) -> &mut dyn OutboxTx;

    /// Makes every write in the transaction durable and visible.
    async fn commit(self: Box<Self>) -> StoreResult<()>;
}

/// What a storage backend supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoreCapabilities {
    /// Short backend name for logs, e.g. `"sqlite"`.
    pub backend: &'static str,
    /// Whether several transactions can write at once. When `false` (SQLite), running more
    /// than one purchase worker per process gains nothing.
    pub concurrent_writers: bool,
}
