use async_trait::async_trait;
use kippu_domain::purchase::{Basket, LineItem, PurchaseRequest, PurchaseStatus};
use kippu_domain::reservation::Reservation;
use kippu_domain::{AccountId, PurchaseRequestId, ReservationId, SaleId, TicketTypeId, Timestamp};

use crate::{Insertion, Keyset, Lease, PageRequest, StoreResult};

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

    /// An account's reservations, newest first (ties in id order), resuming after the position
    /// `page.after` (`created_at` and id of the last reservation).
    async fn reservations_for_account(
        &self,
        account: AccountId,
        page: PageRequest<Keyset>,
    ) -> StoreResult<Vec<Reservation>>;

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

/// Line items in lock order: sorted by ticket type, one line per type, every quantity
/// positive. [`InventoryTx`] takes only these, so no caller can lock rows out of order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LineItems(Vec<LineItem>);

impl LineItems {
    /// Sorts the lines, merges lines of the same type and drops empty ones.
    pub fn new(lines: impl IntoIterator<Item = LineItem>) -> Self {
        let mut lines: Vec<LineItem> = lines.into_iter().filter(|line| line.quantity > 0).collect();
        lines.sort_by_key(|line| line.ticket_type_id);
        let mut merged: Vec<LineItem> = Vec::with_capacity(lines.len());
        for line in lines {
            match merged.last_mut() {
                // Quantities count tickets of one type, which never exceed its u32 capacity.
                Some(last) if last.ticket_type_id == line.ticket_type_id => {
                    last.quantity = last.quantity.saturating_add(line.quantity);
                }
                _ => merged.push(line),
            }
        }
        Self(merged)
    }

    /// One line per ticket type, counting how often each type occurs.
    pub fn count(ticket_types: impl IntoIterator<Item = TicketTypeId>) -> Self {
        Self::new(ticket_types.into_iter().map(|ticket_type_id| LineItem {
            ticket_type_id,
            quantity: 1,
        }))
    }
}

impl From<&Basket> for LineItems {
    /// A basket is canonical already.
    fn from(basket: &Basket) -> Self {
        Self(basket.items().to_vec())
    }
}

impl<'a> IntoIterator for &'a LineItems {
    type Item = &'a LineItem;
    type IntoIter = std::slice::Iter<'a, LineItem>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl std::ops::Deref for LineItems {
    type Target = [LineItem];

    fn deref(&self) -> &[LineItem] {
        &self.0
    }
}

/// Quota claims in lock order, built from [`LineItems`] and each type's per-account limit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Holds(Vec<Hold>);

impl Holds {
    /// Claims for `items` under the limits `limit_of` gives; `None` if a ticket type has none
    /// (it is unknown).
    pub fn new(items: &LineItems, limit_of: impl Fn(TicketTypeId) -> Option<u32>) -> Option<Self> {
        items
            .iter()
            .map(|item| {
                Some(Hold {
                    ticket_type_id: item.ticket_type_id,
                    quantity: item.quantity,
                    per_account_limit: limit_of(item.ticket_type_id)?,
                })
            })
            .collect::<Option<_>>()
            .map(Self)
    }
}

impl<'a> IntoIterator for &'a Holds {
    type Item = &'a Hold;
    type IntoIter = std::slice::Iter<'a, Hold>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl std::ops::Deref for Holds {
    type Target = [Hold];

    fn deref(&self) -> &[Hold] {
        &self.0
    }
}

/// Stock and per-account limits inside a transaction.
///
/// Items arrive as [`LineItems`] and [`Holds`], sorted by ticket type; callers change an
/// account's quota before stock in the same transaction. Both keep lock order consistent and
/// rule out deadlocks between concurrent transactions.
#[async_trait]
pub trait InventoryTx: Send {
    /// Moves `quantity` tickets of every item from available to held.
    ///
    /// **Contract:** linearizable and all-or-nothing. Returns `false` — with no changes — if any
    /// item lacks stock. `held + sold` never exceeds `capacity`.
    async fn try_hold(&mut self, items: &LineItems) -> StoreResult<bool>;

    /// Moves tickets from held back to available (expiry, cancellation).
    async fn release_held(&mut self, items: &LineItems) -> StoreResult<()>;

    /// Moves tickets from held to sold (payment).
    async fn sell_held(&mut self, items: &LineItems) -> StoreResult<()>;

    /// Moves tickets from sold back to available (a refund).
    ///
    /// **Contract:** `sold` never drops below zero.
    async fn return_sold(&mut self, items: &LineItems) -> StoreResult<()>;

    /// Moves tickets straight from available to sold (a late payment re-acquiring stock).
    ///
    /// **Contract:** like [`InventoryTx::try_hold`]: all-or-nothing, never oversells.
    async fn try_sell(&mut self, items: &LineItems) -> StoreResult<bool>;

    /// Counts tickets against the account's per-type limits.
    ///
    /// **Contract:** all-or-nothing. Returns `false` — with no changes — if any limit would be
    /// exceeded.
    async fn try_take_quota(&mut self, account: AccountId, holds: &Holds) -> StoreResult<bool>;

    /// Gives quota back (expiry, cancellation, refund).
    async fn return_quota(&mut self, account: AccountId, items: &LineItems) -> StoreResult<()>;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn line(byte: u8, quantity: u32) -> LineItem {
        LineItem {
            ticket_type_id: TicketTypeId::from(uuid::Uuid::from_bytes([byte; 16])),
            quantity,
        }
    }

    #[test]
    fn line_items_are_sorted_merged_and_never_empty() {
        let items = LineItems::new([line(2, 1), line(1, 2), line(2, 3), line(3, 0)]);
        assert_eq!(&*items, &[line(1, 2), line(2, 4)]);
        let counted =
            LineItems::count([line(2, 1), line(1, 1), line(2, 1)].map(|line| line.ticket_type_id));
        assert_eq!(&*counted, &[line(1, 1), line(2, 2)]);
    }

    #[test]
    fn holds_need_every_limit() {
        let items = LineItems::new([line(1, 2), line(2, 1)]);
        let holds = Holds::new(&items, |_| Some(4)).unwrap();
        assert_eq!(holds.len(), 2);
        assert!(holds.iter().all(|hold| hold.per_account_limit == 4));
        let first = items[0].ticket_type_id;
        assert_eq!(Holds::new(&items, |id| (id == first).then_some(4)), None);
    }
}
