use async_trait::async_trait;
use kippu_domain::ticket::Ticket;
use kippu_domain::{AccountId, ReservationId, TicketId};
use kippu_store::{Keyset, PageRequest, StoreResult, TicketStore, TicketsTx};

use crate::convert::{TicketRow, all, micros, optional, ticket_status};
use crate::tx::MySqlTx;
use crate::{MySqlStore, error};

const TICKET_COLUMNS: &str = "SELECT id, reservation_id, account_id, event_id, ticket_type_id, \
     valid_from, valid_until, issued_at, status, encoded FROM tickets";

#[async_trait]
impl TicketStore for MySqlStore {
    async fn ticket(&self, id: TicketId) -> StoreResult<Option<Ticket>> {
        let row = sqlx::query_as::<_, TicketRow>(sqlx::AssertSqlSafe(format!(
            "{TICKET_COLUMNS} WHERE id = ?"
        )))
        .bind(id.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(error)?;
        optional(row)
    }

    async fn tickets_for_account(
        &self,
        account: AccountId,
        page: PageRequest<Keyset>,
    ) -> StoreResult<Vec<Ticket>> {
        let after_at = page.after.map(|after| micros(after.at));
        let rows = sqlx::query_as::<_, TicketRow>(sqlx::AssertSqlSafe(format!(
            "{TICKET_COLUMNS} WHERE account_id = ?
               AND (? IS NULL OR issued_at < ? OR (issued_at = ? AND id > ?))
             ORDER BY issued_at DESC, id LIMIT ?"
        )))
        .bind(account.as_uuid())
        .bind(after_at)
        .bind(after_at)
        .bind(after_at)
        .bind(page.after.map(|after| after.id))
        .bind(page.limit)
        .fetch_all(&self.pool)
        .await
        .map_err(error)?;
        all(rows)
    }

    async fn tickets_for_reservation(
        &self,
        reservation: ReservationId,
    ) -> StoreResult<Vec<Ticket>> {
        let rows = sqlx::query_as::<_, TicketRow>(sqlx::AssertSqlSafe(format!(
            "{TICKET_COLUMNS} WHERE reservation_id = ? ORDER BY id"
        )))
        .bind(reservation.as_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(error)?;
        all(rows)
    }
}

#[async_trait]
impl TicketsTx for MySqlTx {
    async fn insert_tickets(&mut self, tickets: &[Ticket]) -> StoreResult<()> {
        for ticket in tickets {
            sqlx::query(
                "INSERT INTO tickets (id, reservation_id, account_id, event_id, ticket_type_id,
                                      valid_from, valid_until, issued_at, status, encoded)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(ticket.id.as_uuid())
            .bind(ticket.reservation_id.as_uuid())
            .bind(ticket.account_id.as_uuid())
            .bind(ticket.event_id.as_uuid())
            .bind(ticket.ticket_type_id.as_uuid())
            .bind(micros(ticket.valid_from))
            .bind(micros(ticket.valid_until))
            .bind(micros(ticket.issued_at))
            .bind(ticket_status(ticket.status))
            .bind(ticket.encoded.as_slice())
            .execute(&mut *self.conn)
            .await
            .map_err(error)?;
        }
        Ok(())
    }

    async fn revoke_tickets(&mut self, tickets: &[TicketId]) -> StoreResult<u64> {
        // rows_affected counts matched rows, so the status test must be in the WHERE clause: a
        // ticket another transaction revoked first no longer matches once its lock is released.
        let mut revoked = 0;
        for ticket in tickets {
            revoked += sqlx::query(
                "UPDATE tickets SET status = 'revoked' WHERE id = ? AND status = 'valid'",
            )
            .bind(ticket.as_uuid())
            .execute(&mut *self.conn)
            .await
            .map_err(error)?
            .rows_affected();
        }
        Ok(revoked)
    }
}
