use async_trait::async_trait;
use kippu_domain::ticket::Ticket;
use kippu_domain::{AccountId, ReservationId, TicketId};
use kippu_store::{Keyset, PageRequest, StoreResult, TicketStore, TicketsTx};

use crate::convert::{TicketRow, all, micros, optional, ticket_status};
use crate::tx::SqliteTx;
use crate::{SqliteStore, error};

const TICKET_COLUMNS: &str = "SELECT id, reservation_id, account_id, event_id, ticket_type_id, \
     valid_from, valid_until, issued_at, status, encoded FROM tickets";

#[async_trait]
impl TicketStore for SqliteStore {
    async fn ticket(&self, id: TicketId) -> StoreResult<Option<Ticket>> {
        let row = sqlx::query_as::<_, TicketRow>(sqlx::AssertSqlSafe(format!(
            "{TICKET_COLUMNS} WHERE id = ?1"
        )))
        .bind(id.as_uuid())
        .fetch_optional(&self.reader)
        .await
        .map_err(error)?;
        optional(row)
    }

    async fn tickets_for_account(
        &self,
        account: AccountId,
        page: PageRequest<Keyset>,
    ) -> StoreResult<Vec<Ticket>> {
        let rows = sqlx::query_as::<_, TicketRow>(sqlx::AssertSqlSafe(format!(
            "{TICKET_COLUMNS} WHERE account_id = ?1
               AND (?2 IS NULL OR issued_at < ?2 OR (issued_at = ?2 AND id > ?3))
             ORDER BY issued_at DESC, id LIMIT ?4"
        )))
        .bind(account.as_uuid())
        .bind(page.after.map(|after| micros(after.at)))
        .bind(page.after.map(|after| after.id))
        .bind(page.limit)
        .fetch_all(&self.reader)
        .await
        .map_err(error)?;
        all(rows)
    }

    async fn tickets_for_reservation(
        &self,
        reservation: ReservationId,
    ) -> StoreResult<Vec<Ticket>> {
        let rows = sqlx::query_as::<_, TicketRow>(sqlx::AssertSqlSafe(format!(
            "{TICKET_COLUMNS} WHERE reservation_id = ?1 ORDER BY id"
        )))
        .bind(reservation.as_uuid())
        .fetch_all(&self.reader)
        .await
        .map_err(error)?;
        all(rows)
    }
}

#[async_trait]
impl TicketsTx for SqliteTx {
    async fn insert_tickets(&mut self, tickets: &[Ticket]) -> StoreResult<()> {
        for ticket in tickets {
            sqlx::query(
                "INSERT INTO tickets (id, reservation_id, account_id, event_id, ticket_type_id,
                                      valid_from, valid_until, issued_at, status, encoded)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
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
        let mut revoked = 0;
        for ticket in tickets {
            revoked += sqlx::query(
                "UPDATE tickets SET status = 'revoked' WHERE id = ?1 AND status = 'valid'",
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
