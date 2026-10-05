use async_trait::async_trait;
use kippu_domain::ticket::Ticket;
use kippu_domain::{AccountId, ReservationId, TicketId};

use crate::StoreResult;

/// Issued tickets.
#[async_trait]
pub trait TicketStore {
    /// Looks a ticket up by id.
    async fn ticket(&self, id: TicketId) -> StoreResult<Option<Ticket>>;
    /// An account's tickets, newest first.
    async fn tickets_for_account(&self, account: AccountId) -> StoreResult<Vec<Ticket>>;
    /// The tickets issued for a reservation.
    async fn tickets_for_reservation(&self, reservation: ReservationId)
    -> StoreResult<Vec<Ticket>>;
}

/// Tickets inside a transaction.
#[async_trait]
pub trait TicketsTx: Send {
    /// Records issued tickets.
    async fn insert_tickets(&mut self, tickets: &[Ticket]) -> StoreResult<()>;
}
