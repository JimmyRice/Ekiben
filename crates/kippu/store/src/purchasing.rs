use async_trait::async_trait;
use kippu_domain::purchase::{LineItem, PurchaseRequest, PurchaseStatus};
use kippu_domain::reservation::Reservation;
use kippu_domain::{AccountId, PurchaseRequestId, ReservationId, SaleId, TicketTypeId, Timestamp};

use crate::{Insertion, Lease, StoreResult};

/// Purchase requests and reservations, outside a transaction.
#[async_trait]
pub trait PurchaseStore {
    /// Records a purchase request in the `Queued` state.
    ///
    /// **Contract:** ids are unique. If a request with the same id exists, nothing is written
    /// and the existing request is returned, so submitting a request is idempotent.
    async fn insert_purchase_request(
        &self,
        request: &PurchaseRequest,
    ) -> StoreResult<Insertion<PurchaseRequest>>;

    /// Looks a purchase request up by id.
    async fn purchase_request(&self, id: PurchaseRequestId)
    -> StoreResult<Option<PurchaseRequest>>;

    /// Claims up to `limit` queued requests whose previous lease (if any) has lapsed, oldest
    /// first, and leases them until `lease.until`.
    ///
    /// **Contract:** while a lease is live, no other caller can claim the same request.
    async fn claim_purchase_requests(
        &self,
        lease: Lease,
        limit: u32,
    ) -> StoreResult<Vec<PurchaseRequest>>;

    /// How many requests of a sale are still queued: the backlog the waiting room watches.
    async fn queued_purchase_count(&self, sale: SaleId) -> StoreResult<u64>;

    /// Looks a reservation up by id.
    async fn reservation(&self, id: ReservationId) -> StoreResult<Option<Reservation>>;

    /// An account's reservations, newest first.
    async fn reservations_for_account(&self, account: AccountId) -> StoreResult<Vec<Reservation>>;

    /// Reservations still holding inventory whose `expires_at` is at or before `now`.
    async fn overdue_reservations(
        &self,
        now: Timestamp,
        limit: u32,
    ) -> StoreResult<Vec<ReservationId>>;
}

/// Purchase requests inside a transaction.
#[async_trait]
pub trait PurchasesTx: Send {
    /// Reads a request and locks it until the transaction ends.
    async fn lock_purchase_request(
        &mut self,
        id: PurchaseRequestId,
    ) -> StoreResult<Option<PurchaseRequest>>;

    /// Records the outcome of a request.
    async fn set_purchase_status(
        &mut self,
        id: PurchaseRequestId,
        status: PurchaseStatus,
        now: Timestamp,
    ) -> StoreResult<()>;
}

/// A per-account limit to respect while taking quota.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hold {
    /// The ticket type.
    pub ticket_type_id: TicketTypeId,
    /// How many tickets.
    pub quantity: u32,
    /// The most tickets of this type one account may hold.
    pub per_account_limit: u32,
}

/// Stock and per-account limits inside a transaction.
///
/// Callers pass items sorted by ticket type, which keeps lock order consistent and rules out
/// deadlocks between concurrent transactions.
#[async_trait]
pub trait InventoryTx: Send {
    /// Moves `quantity` tickets of every item from available to held.
    ///
    /// **Contract:** linearizable and all-or-nothing. Returns `false` — with no changes — if any
    /// item lacks stock. `held + sold` never exceeds `capacity`.
    async fn try_hold(&mut self, items: &[LineItem]) -> StoreResult<bool>;

    /// Moves tickets from held back to available (expiry, cancellation).
    async fn release_held(&mut self, items: &[LineItem]) -> StoreResult<()>;

    /// Moves tickets from held to sold (payment).
    async fn sell_held(&mut self, items: &[LineItem]) -> StoreResult<()>;

    /// Moves tickets straight from available to sold (a late payment re-acquiring stock).
    ///
    /// **Contract:** like [`InventoryTx::try_hold`]: all-or-nothing, never oversells.
    async fn try_sell(&mut self, items: &[LineItem]) -> StoreResult<bool>;

    /// Counts tickets against the account's per-type limits.
    ///
    /// **Contract:** all-or-nothing. Returns `false` — with no changes — if any limit would be
    /// exceeded.
    async fn try_take_quota(&mut self, account: AccountId, holds: &[Hold]) -> StoreResult<bool>;

    /// Gives quota back (expiry, cancellation, refund).
    async fn return_quota(&mut self, account: AccountId, items: &[LineItem]) -> StoreResult<()>;
}

/// Reservations inside a transaction.
#[async_trait]
pub trait ReservationsTx: Send {
    /// Records a new reservation.
    ///
    /// **Contract:** at most one reservation per purchase request; a second insert fails with
    /// `Conflict("purchase_request_id")`.
    async fn insert_reservation(&mut self, reservation: &Reservation) -> StoreResult<()>;

    /// Reads a reservation and locks it until the transaction ends.
    async fn lock_reservation(&mut self, id: ReservationId) -> StoreResult<Option<Reservation>>;

    /// Writes a reservation's status, attestor, expiry and update time.
    async fn update_reservation(&mut self, reservation: &Reservation) -> StoreResult<()>;
}
