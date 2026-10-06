#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

mod accounts;
mod catalog;
#[cfg(feature = "conformance")]
pub mod conformance;
mod error;
mod housekeeping;
mod images;
mod outbox;
mod payments;
mod primitives;
mod purchasing;
mod queue;
mod store;
mod ticketing;
mod webhooks;

pub use accounts::{AccountStore, Session, SessionRenewal, Unlink};
pub use catalog::{CatalogStore, EventFilter};
pub use error::{BoxError, StoreError, StoreResult};
pub use housekeeping::{AuditEntry, HousekeepingStore, IdempotencyRecord};
pub use images::ImageStore;
pub use outbox::{OutboxRecord, OutboxStore, OutboxTx};
pub use payments::{PaymentStore, PaymentsTx};
pub use primitives::{Insertion, Lease, PageRequest};
pub use purchasing::{Hold, InventoryTx, PurchaseStore, PurchasesTx, ReservationsTx};
pub use queue::{EventBus, InboxDelivery, PurchaseInbox};
pub use store::{Store, StoreCapabilities, StoreTx};
pub use ticketing::{TicketStore, TicketsTx};
pub use webhooks::{WebhookRun, WebhookStore};
