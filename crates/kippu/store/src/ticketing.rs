use async_trait::async_trait;
use kippu_domain::ticket::Ticket;
use kippu_domain::{AccountId, ReservationId, TicketId};

use crate::{Keyset, PageRequest, StoreResult};

/// Issued tickets.
#[async_trait]
pub trait TicketStore {
    /// Looks a ticket up by id.
    async fn ticket(&self, id: TicketId) -> StoreResult<Option<Ticket>>;
    /// An account's tickets, most recently issued first (tickets issued together in id order),
    /// resuming after the position `page.after` (`issued_at` and id of the last ticket).
    async fn tickets_for_account(
        &self,
        account: AccountId,
        page: PageRequest<Keyset>,
    ) -> StoreResult<Vec<Ticket>>;
    /// The tickets issued for a reservation.
    async fn tickets_for_reservation(&self, reservation: ReservationId)
    -> StoreResult<Vec<Ticket>>;
}

/// Tickets inside a transaction.
#[async_trait]
pub trait TicketsTx: Send {
    /// Records issued tickets.
    async fn insert_tickets(&mut self, tickets: &[Ticket]) -> StoreResult<()>;

    /// Revokes tickets that are still valid, returning how many it revoked. Callers pass ids
    /// sorted, which keeps lock order consistent between concurrent transactions.
    ///
    /// **Contract:** only `valid` tickets change, so of concurrent transactions revoking the same
    /// ticket exactly one counts it: a caller that revoked fewer tickets than it named lost a
    /// race and must not act as if it revoked them all.
    async fn revoke_tickets(&mut self, tickets: &[TicketId]) -> StoreResult<u64>;
}
