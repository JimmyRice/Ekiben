use async_trait::async_trait;
use kippu_domain::admission::WaitingRoom;
use kippu_domain::catalog::{Event, Inventory, Sale, TicketType};
use kippu_domain::{AccountId, EventId, OrganizationId, SaleId, TicketTypeId, Timestamp};

use crate::{PageRequest, StoreResult};

/// Which events to list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EventFilter {
    /// Only events of this organization.
    pub organization: Option<OrganizationId>,
    /// Only events the public may see.
    pub public_only: bool,
}

/// Events, sales, ticket types, inventory, favourites and waiting rooms.
///
/// Updates take the version the caller read and fail with `Conflict("version")` if the record
/// changed since (optimistic concurrency). The stored version becomes `record.version`.
#[async_trait]
pub trait CatalogStore {
    /// Creates an event. Fails with `Conflict("slug")` if the slug is taken.
    async fn insert_event(&self, event: &Event) -> StoreResult<()>;
    /// Replaces an event if its stored version is still `expected_version`.
    async fn update_event(&self, event: &Event, expected_version: i64) -> StoreResult<()>;
    /// Looks an event up by id.
    async fn event(&self, id: EventId) -> StoreResult<Option<Event>>;
    /// Lists events in id order.
    async fn list_events(&self, filter: EventFilter, page: PageRequest) -> StoreResult<Vec<Event>>;

    /// Creates a sale.
    async fn insert_sale(&self, sale: &Sale) -> StoreResult<()>;
    /// Replaces a sale if its stored version is still `expected_version`.
    async fn update_sale(&self, sale: &Sale, expected_version: i64) -> StoreResult<()>;
    /// Looks a sale up by id.
    async fn sale(&self, id: SaleId) -> StoreResult<Option<Sale>>;
    /// The sales of an event, in id order.
    async fn list_sales(&self, event: EventId) -> StoreResult<Vec<Sale>>;

    /// Creates a ticket type together with its inventory (`held = sold = 0`), atomically.
    async fn insert_ticket_type(&self, ticket_type: &TicketType) -> StoreResult<()>;
    /// Replaces a ticket type if its stored version is still `expected_version`. A capacity
    /// change is applied to the inventory atomically and fails with `Conflict("capacity")` if
    /// it would drop below `held + sold`.
    async fn update_ticket_type(
        &self,
        ticket_type: &TicketType,
        expected_version: i64,
    ) -> StoreResult<()>;
    /// Looks a ticket type up by id.
    async fn ticket_type(&self, id: TicketTypeId) -> StoreResult<Option<TicketType>>;
    /// The ticket types of a sale, in id order.
    async fn list_ticket_types(&self, sale: SaleId) -> StoreResult<Vec<TicketType>>;
    /// Current stock of a ticket type.
    async fn inventory(&self, ticket_type: TicketTypeId) -> StoreResult<Option<Inventory>>;

    /// Marks an event as a favourite of an account. Idempotent.
    async fn add_favorite(
        &self,
        account: AccountId,
        event: EventId,
        now: Timestamp,
    ) -> StoreResult<()>;
    /// Removes a favourite. Idempotent.
    async fn remove_favorite(&self, account: AccountId, event: EventId) -> StoreResult<()>;
    /// An account's favourite events.
    async fn favorites(&self, account: AccountId) -> StoreResult<Vec<EventId>>;

    /// Hands out the next queue position of a sale's waiting room (starting at 1).
    ///
    /// **Contract:** concurrent callers always receive distinct positions.
    async fn join_waiting_room(&self, sale: SaleId) -> StoreResult<u64>;
    /// The waiting room's progress (all zeros if nobody joined yet).
    async fn waiting_room(&self, sale: SaleId) -> StoreResult<WaitingRoom>;
    /// Moves `admitted_through` from `from` to `to`.
    ///
    /// **Contract:** a compare-and-set: returns `false` without changes if another instance
    /// already moved it, so concurrent tickers never admit a batch twice.
    async fn advance_waiting_room(&self, sale: SaleId, from: u64, to: u64) -> StoreResult<bool>;
}
