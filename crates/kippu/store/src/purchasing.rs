use async_trait::async_trait;
use kippu_domain::purchase::{Basket, LineItem, PurchaseRequest, PurchaseStatus};
use kippu_domain::reservation::{Reservation, ReservationStatus};
use kippu_domain::{
    AccountId, EventId, OrganizationId, PurchaseRequestId, ReservationId, SaleId, TicketTypeId,
    Timestamp,
};

use crate::{Insertion, Keyset, PageRequest, StoreResult};

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

    /// Sales with queued requests that are due by `now` (not postponed beyond it), in the
    /// order of their oldest such request, looking at no more than the `limit` oldest requests;
    /// each with how many of those requests are its.
    async fn sales_with_queued_requests(
        &self,
        now: Timestamp,
        limit: u32,
    ) -> StoreResult<Vec<(SaleId, u32)>>;

    /// Keeps a queued request from being served before `until`: a request that failed on its
    /// own waits before it is tried again instead of holding up the requests behind it.
    async fn postpone_purchase_request(
        &self,
        id: PurchaseRequestId,
        until: Timestamp,
    ) -> StoreResult<()>;

    /// How many requests of a sale are still queued, counting no further than `cap`: the
    /// backlog the waiting room compares with its limit.
    ///
    /// **Contract:** returns `min(queued, cap)`, reading at most `cap` rows however long the
    /// queue is.
    async fn queued_purchase_count(&self, sale: SaleId, cap: u32) -> StoreResult<u64>;

    /// Looks a reservation up by id.
    async fn reservation(&self, id: ReservationId) -> StoreResult<Option<Reservation>>;

    /// An account's reservations, newest first (ties in id order), resuming after the position
    /// `page.after` (`created_at` and id of the last reservation).
    async fn reservations_for_account(
        &self,
        account: AccountId,
        page: PageRequest<Keyset>,
    ) -> StoreResult<Vec<Reservation>>;

    /// An event's reservations, all of them or those in `status`, newest first (ties in id
    /// order), resuming after the position `page.after` (`created_at` and id of the last
    /// reservation).
    async fn reservations_for_event(
        &self,
        event: EventId,
        status: Option<ReservationStatus>,
        page: PageRequest<Keyset>,
    ) -> StoreResult<Vec<Reservation>>;

    /// Whether anyone (or `account`) ordered for an event: a reservation that has not expired
    /// or been cancelled, so tickets are held, sold, or were paid for.
    async fn has_orders(&self, event: EventId, account: Option<AccountId>) -> StoreResult<bool>;

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
    /// Takes a sale's purchase queue for this transaction, unless another open transaction
    /// holds it: then returns `false` at once, without waiting.
    ///
    /// **Contract:** at most one open transaction holds a sale's queue, and it lets go as the
    /// transaction ends, however it ends (commit, rollback, a dropped connection), so a worker
    /// that dies holds nothing up. A backend whose write transactions never overlap may always
    /// return `true`.
    async fn lock_purchase_queue(&mut self, sale: SaleId) -> StoreResult<bool>;

    /// The oldest queued requests of a sale that are due by `now`, at most `limit`, in arrival
    /// order (`created_at`, ties by id), each locked until the transaction ends, and whether
    /// each one's account is denied for `event` of `organization` (as
    /// [`DenialStore::account_denied`](crate::DenialStore::account_denied) answers).
    ///
    /// **Contract:** read after [`PurchasesTx::lock_purchase_queue`], the requests are exactly
    /// those no transaction that held the queue before has served.
    async fn queued_requests(
        &mut self,
        sale: SaleId,
        event: EventId,
        organization: OrganizationId,
        now: Timestamp,
        limit: u32,
    ) -> StoreResult<Vec<QueuedRequest>>;

    /// Records the outcome of a request.
    async fn set_purchase_status(
        &mut self,
        id: PurchaseRequestId,
        status: PurchaseStatus,
        now: Timestamp,
    ) -> StoreResult<()>;

    /// Records the outcomes of several requests, each named once, as
    /// [`PurchasesTx::set_purchase_status`] would one by one, in a few statements however many
    /// there are.
    async fn set_purchase_statuses(
        &mut self,
        outcomes: &[(PurchaseRequestId, PurchaseStatus)],
        now: Timestamp,
    ) -> StoreResult<()>;
}

/// A queued request as a purchase worker reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedRequest {
    /// The request.
    pub request: PurchaseRequest,
    /// Whether its account is denied for the sale's event.
    pub account_denied: bool,
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

/// Tickets of one type counted against one account's per-account limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuotaLine {
    /// The account.
    pub account_id: AccountId,
    /// The ticket type.
    pub ticket_type_id: TicketTypeId,
    /// How many tickets.
    pub quantity: u32,
}

/// Quota lines in lock order: sorted by account, then ticket type, one line per pair, every
/// quantity positive. [`InventoryTx::lock_quotas`] and [`InventoryTx::take_quotas`] take only
/// these, so several accounts' quotas are locked in the order single accounts take theirs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QuotaLines(Vec<QuotaLine>);

impl QuotaLines {
    /// Sorts the lines, merges lines of the same account and type and drops empty ones.
    pub fn new(lines: impl IntoIterator<Item = QuotaLine>) -> Self {
        let mut lines: Vec<QuotaLine> =
            lines.into_iter().filter(|line| line.quantity > 0).collect();
        lines.sort_by_key(|line| (line.account_id, line.ticket_type_id));
        let mut merged: Vec<QuotaLine> = Vec::with_capacity(lines.len());
        for line in lines {
            match merged.last_mut() {
                // Quantities count tickets of one type, which never exceed its u32 capacity.
                Some(last)
                    if (last.account_id, last.ticket_type_id)
                        == (line.account_id, line.ticket_type_id) =>
                {
                    last.quantity = last.quantity.saturating_add(line.quantity);
                }
                _ => merged.push(line),
            }
        }
        Self(merged)
    }

    /// The lines of one account's items.
    pub fn of(account: AccountId, items: &LineItems) -> impl Iterator<Item = QuotaLine> + '_ {
        items.iter().map(move |item| QuotaLine {
            account_id: account,
            ticket_type_id: item.ticket_type_id,
            quantity: item.quantity,
        })
    }
}

impl<'a> IntoIterator for &'a QuotaLines {
    type Item = &'a QuotaLine;
    type IntoIter = std::slice::Iter<'a, QuotaLine>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl std::ops::Deref for QuotaLines {
    type Target = [QuotaLine];

    fn deref(&self) -> &[QuotaLine] {
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
    /// Locks the stock of `items`' ticket types, in ticket type order, until the transaction
    /// ends, and returns how many tickets of each are available (capacity less held and sold),
    /// in the items' order; 0 for a type without stock. Quantities do not matter here. A
    /// transaction that decides several purchases at once reads their stock with this, decides
    /// them one after another on what it read, and holds what they take with one
    /// [`InventoryTx::try_hold`].
    ///
    /// **Contract:** two transactions that lock overlapping stock this way before changing it
    /// never deadlock on it, and what a transaction reads stays as read until it changes it
    /// itself. A backend with a single writer only reads.
    async fn lock_stock(&mut self, items: &LineItems) -> StoreResult<Vec<u32>>;

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

    /// Locks the quotas `lines` name, in their order, until the transaction ends, and returns
    /// how many tickets each line's account has taken of its type, in the lines' order; 0 for a
    /// quota never taken. Quantities do not matter here. A transaction that decides several
    /// accounts' purchases at once reads their quotas with this, decides on what it read, and
    /// counts what they take with [`InventoryTx::take_quotas`].
    ///
    /// **Contract:** what a transaction reads stays as read until it changes it itself,
    /// quotas never taken before included, so decisions made on these reads never exceed a
    /// limit however many transactions decide at once; two transactions that lock overlapping
    /// quotas this way never deadlock on them. A backend with a single writer only reads.
    async fn lock_quotas(&mut self, lines: &QuotaLines) -> StoreResult<Vec<u32>>;

    /// Counts tickets against accounts' quotas without checking any limit: the caller decided
    /// them on what [`InventoryTx::lock_quotas`] read in this transaction. One statement for
    /// all of them on a backend that allows it.
    async fn take_quotas(&mut self, lines: &QuotaLines) -> StoreResult<()>;
}

/// Reservations inside a transaction.
#[async_trait]
pub trait ReservationsTx: Send {
    /// Records a new reservation.
    ///
    /// **Contract:** at most one reservation per purchase request; a second insert fails with
    /// `Conflict("purchase_request_id")`.
    async fn insert_reservation(&mut self, reservation: &Reservation) -> StoreResult<()>;

    /// Records several new reservations, as [`ReservationsTx::insert_reservation`] would one by
    /// one, in a few statements however many there are.
    ///
    /// **Contract:** if any is for a purchase request that already has a reservation (or two
    /// are for the same one), the call fails with `Conflict("purchase_request_id")`, and none
    /// of them is recorded once the transaction is dropped.
    async fn insert_reservations(&mut self, reservations: &[Reservation]) -> StoreResult<()>;

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
    fn quota_lines_are_sorted_by_account_then_type_merged_and_never_empty() {
        let account = |byte: u8| AccountId::from(uuid::Uuid::from_bytes([byte; 16]));
        let quota = |account_byte: u8, line: LineItem| QuotaLine {
            account_id: account(account_byte),
            ticket_type_id: line.ticket_type_id,
            quantity: line.quantity,
        };
        let lines = QuotaLines::new([
            quota(2, line(1, 1)),
            quota(1, line(2, 2)),
            quota(1, line(1, 1)),
            quota(2, line(1, 3)),
            quota(1, line(3, 0)),
        ]);
        assert_eq!(
            &*lines,
            &[
                quota(1, line(1, 1)),
                quota(1, line(2, 2)),
                quota(2, line(1, 4))
            ]
        );
        let items = LineItems::new([line(2, 1), line(1, 2)]);
        assert_eq!(
            QuotaLines::of(account(7), &items).collect::<Vec<_>>(),
            [quota(7, line(1, 2)), quota(7, line(2, 1))]
        );
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
