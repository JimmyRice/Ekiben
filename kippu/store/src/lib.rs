#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

mod accounts;
mod catalog;
#[cfg(feature = "conformance")]
pub mod conformance;
mod error;
mod housekeeping;
mod outbox;
mod payments;
mod purchasing;
mod ticketing;

use async_trait::async_trait;
use kippu_domain::Timestamp;

pub use accounts::{AccountStore, Session, SessionRenewal};
pub use catalog::{CatalogStore, EventFilter};
pub use error::{BoxError, StoreError};
pub use housekeeping::{AuditEntry, HousekeepingStore, IdempotencyRecord};
pub use outbox::{OutboxRecord, OutboxStore, OutboxTx};
pub use payments::{PaymentStore, PaymentsTx};
pub use purchasing::{Hold, InventoryTx, PurchaseStore, PurchasesTx, ReservationsTx};
pub use ticketing::{TicketStore, TicketsTx};

/// Result of a storage operation.
pub type StoreResult<T> = Result<T, StoreError>;

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

/// The outcome of inserting a record under a unique key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Insertion<T> {
    /// The record is new.
    Inserted,
    /// A record with the same key already exists; here it is.
    Existing(T),
}

/// Keyset pagination: up to `limit` records ordered by id, starting after `after`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageRequest {
    /// Maximum number of records.
    pub limit: u32,
    /// Return records whose id sorts after this one.
    pub after: Option<uuid::Uuid>,
}

impl PageRequest {
    /// The first page of the given size.
    pub const fn first(limit: u32) -> Self {
        Self { limit, after: None }
    }
}

impl Default for PageRequest {
    fn default() -> Self {
        Self::first(50)
    }
}

/// A lease on queued work, held until `until` by whichever worker claimed it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lease {
    /// When the claim is taken.
    pub now: Timestamp,
    /// When the claim lapses if the worker dies without finishing.
    pub until: Timestamp,
}
