use async_trait::async_trait;
use kippu_domain::admission::WaitingRoom;
use kippu_domain::catalog::{Event, EventStatus, EventSummary, Inventory, Sale, TicketType};
use kippu_domain::validation::CountryCode;
use kippu_domain::{AccountId, EventId, OrganizationId, SaleId, TicketTypeId, Timestamp};

use crate::{Keyset, PageRequest, StoreResult};

/// Which events to list. Every condition that is set must hold.
///
/// Times bound half-open ranges: `*_from` is inclusive, `*_before` exclusive.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EventFilter {
    /// Only events of this organization.
    pub organization: Option<OrganizationId>,
    /// Only events the public may see.
    pub public_only: bool,
    /// Only events in this status.
    pub status: Option<EventStatus>,
    /// Only events whose address is in this country.
    pub country: Option<CountryCode>,
    /// Only events starting at or after this instant.
    pub starts_from: Option<Timestamp>,
    /// Only events starting before this instant.
    pub starts_before: Option<Timestamp>,
    /// Only events ending at or after this instant.
    pub ends_from: Option<Timestamp>,
    /// Only events ending before this instant.
    pub ends_before: Option<Timestamp>,
}

/// The order events are listed in. Ties are broken by id, in the same direction, so every
/// order is total and a [`Keyset`] of `(sort time, id)` resumes it exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum EventOrder {
    /// Earliest start first.
    #[default]
    StartsAt,
    /// Latest start first.
    StartsAtDesc,
    /// Oldest first.
    CreatedAt,
    /// Newest first.
    CreatedAtDesc,
}

impl EventOrder {
    /// The position of `event` in this order.
    pub fn position(self, event: &EventSummary) -> Keyset {
        let at = match self {
            Self::StartsAt | Self::StartsAtDesc => event.starts_at,
            Self::CreatedAt | Self::CreatedAtDesc => event.created_at,
        };
        Keyset {
            at,
            id: event.id.as_uuid(),
        }
    }
}

/// An event an account marked as a favourite, and when.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Favorite {
    /// The event.
    pub event_id: EventId,
    /// When it was marked.
    pub created_at: Timestamp,
}

impl Favorite {
    /// The position of this favourite in an account's favourites.
    pub fn position(&self) -> Keyset {
        Keyset {
            at: self.created_at,
            id: self.event_id.as_uuid(),
        }
    }
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
    /// Looks an event up by id, without its content: what checks of who may see or edit it
    /// need, at a fraction of the read.
    async fn event_summary(&self, id: EventId) -> StoreResult<Option<EventSummary>>;
    /// Lists the events matching `filter` in `order`, without their content, resuming after
    /// the position `page.after` (the sort time and id of [`EventOrder::position`]).
    async fn list_events(
        &self,
        filter: &EventFilter,
        order: EventOrder,
        page: PageRequest<Keyset>,
    ) -> StoreResult<Vec<EventSummary>>;

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
    /// Current stock of every ticket type of a sale, in ticket type order: one read for what
    /// [`CatalogStore::inventory`] would need one per type for.
    async fn sale_inventory(&self, sale: SaleId) -> StoreResult<Vec<Inventory>>;

    /// Marks an event as a favourite of an account. Idempotent.
    async fn add_favorite(
        &self,
        account: AccountId,
        event: EventId,
        now: Timestamp,
    ) -> StoreResult<()>;
    /// Removes a favourite. Idempotent.
    async fn remove_favorite(&self, account: AccountId, event: EventId) -> StoreResult<()>;
    /// An account's favourite events, most recently marked first (ties by event id),
    /// resuming after the position `page.after` (see [`Favorite::position`]).
    async fn favorites(
        &self,
        account: AccountId,
        page: PageRequest<Keyset>,
    ) -> StoreResult<Vec<Favorite>>;

    /// Hands out the next queue position of a sale's waiting room (starting at 1).
    ///
    /// **Contract:** concurrent callers always receive distinct positions.
    async fn join_waiting_room(&self, sale: SaleId) -> StoreResult<u64>;
    /// The waiting room's progress (all zeros if nobody joined yet).
    async fn waiting_room(&self, sale: SaleId) -> StoreResult<WaitingRoom>;
    /// Sales whose waiting room still has people waiting (`admitted_through < last_position`).
    async fn pending_waiting_rooms(&self) -> StoreResult<Vec<SaleId>>;
    /// Moves `admitted_through` from `from` to `to`.
    ///
    /// **Contract:** a compare-and-set: returns `false` without changes if another instance
    /// already moved it, so concurrent tickers never admit a batch twice.
    async fn advance_waiting_room(&self, sale: SaleId, from: u64, to: u64) -> StoreResult<bool>;
}
