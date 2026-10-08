use async_trait::async_trait;
use kippu_domain::purchase::{LineItem, PurchaseRequest, PurchaseStatus};
use kippu_domain::reservation::Reservation;
use kippu_domain::{AccountId, PurchaseRequestId, ReservationId, SaleId, Timestamp};
use kippu_store::{
    Hold, Holds, Insertion, InventoryTx, Keyset, Lease, LineItems, PageRequest, PurchaseStore,
    PurchasesTx, ReservationsTx, StoreError, StoreResult,
};
use uuid::Uuid;

use crate::convert::{
    PurchaseRequestRow, ReservationRow, all, json, micros, optional, purchase_status,
};
use crate::tx::SqliteTx;
use crate::{SqliteStore, error, unique};

/// A query whose parameters are still being bound.
type SqliteQuery<'q> = sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments>;

const PURCHASE_REQUEST_COLUMNS: &str = "id, account_id, sale_id, basket, status, reservation_id, rejection_reason, created_at, updated_at";

const RESERVATION_COLUMNS: &str = "SELECT id, purchase_request_id, account_id, sale_id, event_id, \
     items, total_minor, currency, environment, status, attestor_id, expires_at, created_at, \
     updated_at FROM reservations";

#[async_trait]
impl PurchaseStore for SqliteStore {
    async fn insert_purchase_request(
        &self,
        request: &PurchaseRequest,
    ) -> StoreResult<Insertion<PurchaseRequest>> {
        let status = purchase_status(request.status);
        let inserted = sqlx::query(
            "INSERT INTO purchase_requests (id, account_id, sale_id, basket, status, reservation_id,
                                            rejection_reason, lease_until, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, ?8, ?9)
             ON CONFLICT (id) DO NOTHING",
        )
        .bind(request.id.as_uuid())
        .bind(request.account_id.as_uuid())
        .bind(request.sale_id.as_uuid())
        .bind(json(&request.basket)?)
        .bind(status.status)
        .bind(status.reservation_id)
        .bind(status.rejection_reason)
        .bind(micros(request.created_at))
        .bind(micros(request.updated_at))
        .execute(&self.writer)
        .await
        .map_err(error)?;
        if inserted.rows_affected() == 1 {
            return Ok(Insertion::Inserted);
        }
        let existing = sqlx::query_as::<_, PurchaseRequestRow>(sqlx::AssertSqlSafe(format!(
            "SELECT {PURCHASE_REQUEST_COLUMNS} FROM purchase_requests WHERE id = ?1"
        )))
        .bind(request.id.as_uuid())
        .fetch_one(&self.writer)
        .await
        .map_err(error)?;
        Ok(Insertion::Existing(existing.try_into()?))
    }

    async fn purchase_request(
        &self,
        id: PurchaseRequestId,
    ) -> StoreResult<Option<PurchaseRequest>> {
        let row = sqlx::query_as::<_, PurchaseRequestRow>(sqlx::AssertSqlSafe(format!(
            "SELECT {PURCHASE_REQUEST_COLUMNS} FROM purchase_requests WHERE id = ?1"
        )))
        .bind(id.as_uuid())
        .fetch_optional(&self.reader)
        .await
        .map_err(error)?;
        optional(row)
    }

    async fn claim_purchase_requests(
        &self,
        lease: Lease,
        limit: u32,
    ) -> StoreResult<Vec<PurchaseRequest>> {
        let rows = sqlx::query_as::<_, PurchaseRequestRow>(sqlx::AssertSqlSafe(format!(
            "UPDATE purchase_requests SET lease_until = ?1
             WHERE id IN (SELECT id FROM purchase_requests
                          WHERE status = 'queued' AND (lease_until IS NULL OR lease_until <= ?2)
                          ORDER BY created_at LIMIT ?3)
             RETURNING {PURCHASE_REQUEST_COLUMNS}"
        )))
        .bind(micros(lease.until))
        .bind(micros(lease.now))
        .bind(limit)
        .fetch_all(&self.writer)
        .await
        .map_err(error)?;
        let mut requests: Vec<PurchaseRequest> = all(rows)?;
        requests.sort_by_key(|request| request.created_at);
        Ok(requests)
    }

    async fn queued_purchase_count(&self, sale: SaleId) -> StoreResult<u64> {
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM purchase_requests WHERE sale_id = ?1 AND status = 'queued'",
        )
        .bind(sale.as_uuid())
        .fetch_one(&self.reader)
        .await
        .map_err(error)?;
        u64::try_from(count).map_err(StoreError::backend)
    }

    async fn reservation(&self, id: ReservationId) -> StoreResult<Option<Reservation>> {
        let row = sqlx::query_as::<_, ReservationRow>(sqlx::AssertSqlSafe(format!(
            "{RESERVATION_COLUMNS} WHERE id = ?1"
        )))
        .bind(id.as_uuid())
        .fetch_optional(&self.reader)
        .await
        .map_err(error)?;
        optional(row)
    }

    async fn reservations_for_account(
        &self,
        account: AccountId,
        page: PageRequest<Keyset>,
    ) -> StoreResult<Vec<Reservation>> {
        let rows = sqlx::query_as::<_, ReservationRow>(sqlx::AssertSqlSafe(format!(
            "{RESERVATION_COLUMNS} WHERE account_id = ?1
               AND (?2 IS NULL OR created_at < ?2 OR (created_at = ?2 AND id > ?3))
             ORDER BY created_at DESC, id LIMIT ?4"
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

    async fn overdue_reservations(
        &self,
        now: Timestamp,
        limit: u32,
    ) -> StoreResult<Vec<ReservationId>> {
        let ids = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM reservations
             WHERE status IN ('reserved', 'payment_pending') AND expires_at <= ?1
             ORDER BY expires_at LIMIT ?2",
        )
        .bind(micros(now))
        .bind(limit)
        .fetch_all(&self.reader)
        .await
        .map_err(error)?;
        Ok(ids.into_iter().map(Into::into).collect())
    }
}

#[async_trait]
impl PurchasesTx for SqliteTx {
    async fn lock_purchase_request(
        &mut self,
        id: PurchaseRequestId,
    ) -> StoreResult<Option<PurchaseRequest>> {
        // The transaction already holds SQLite's single write lock; nothing finer is needed.
        let row = sqlx::query_as::<_, PurchaseRequestRow>(sqlx::AssertSqlSafe(format!(
            "SELECT {PURCHASE_REQUEST_COLUMNS} FROM purchase_requests WHERE id = ?1"
        )))
        .bind(id.as_uuid())
        .fetch_optional(&mut *self.conn)
        .await
        .map_err(error)?;
        optional(row)
    }

    async fn set_purchase_status(
        &mut self,
        id: PurchaseRequestId,
        status: PurchaseStatus,
        now: Timestamp,
    ) -> StoreResult<()> {
        let columns = purchase_status(status);
        sqlx::query(
            "UPDATE purchase_requests
             SET status = ?2, reservation_id = ?3, rejection_reason = ?4, lease_until = NULL,
                 updated_at = ?5
             WHERE id = ?1",
        )
        .bind(id.as_uuid())
        .bind(columns.status)
        .bind(columns.reservation_id)
        .bind(columns.rejection_reason)
        .bind(micros(now))
        .execute(&mut *self.conn)
        .await
        .map_err(error)?;
        Ok(())
    }
}

impl SqliteTx {
    /// Runs a conditional update per item. If any affects no row, reverts the ones that did with
    /// `undo` and returns `false`, so the operation as a whole is all-or-nothing.
    async fn all_or_nothing<T: Copy + Send + Sync>(
        &mut self,
        items: &[T],
        apply: &'static str,
        undo: &'static str,
        bind: impl Fn(SqliteQuery<'_>, T) -> SqliteQuery<'_> + Send + Sync,
    ) -> StoreResult<bool> {
        for (applied, &item) in items.iter().enumerate() {
            let result = bind(sqlx::query(apply), item)
                .execute(&mut *self.conn)
                .await
                .map_err(error)?;
            if result.rows_affected() == 0 {
                for &done in items.iter().take(applied) {
                    bind(sqlx::query(undo), done)
                        .execute(&mut *self.conn)
                        .await
                        .map_err(error)?;
                }
                return Ok(false);
            }
        }
        Ok(true)
    }

    async fn for_each_item(
        &mut self,
        items: &[LineItem],
        statement: &'static str,
    ) -> StoreResult<()> {
        for item in items {
            sqlx::query(statement)
                .bind(item.ticket_type_id.as_uuid())
                .bind(item.quantity)
                .execute(&mut *self.conn)
                .await
                .map_err(error)?;
        }
        Ok(())
    }
}

fn bind_item(query: SqliteQuery<'_>, item: LineItem) -> SqliteQuery<'_> {
    query
        .bind(item.ticket_type_id.as_uuid())
        .bind(item.quantity)
}

#[async_trait]
impl InventoryTx for SqliteTx {
    async fn try_hold(&mut self, items: &LineItems) -> StoreResult<bool> {
        self.all_or_nothing(
            items,
            "UPDATE inventory SET held = held + ?2
             WHERE ticket_type_id = ?1 AND held + sold + ?2 <= capacity",
            "UPDATE inventory SET held = held - ?2 WHERE ticket_type_id = ?1",
            bind_item,
        )
        .await
    }

    async fn release_held(&mut self, items: &LineItems) -> StoreResult<()> {
        self.for_each_item(
            items,
            "UPDATE inventory SET held = held - ?2 WHERE ticket_type_id = ?1",
        )
        .await
    }

    async fn sell_held(&mut self, items: &LineItems) -> StoreResult<()> {
        self.for_each_item(
            items,
            "UPDATE inventory SET held = held - ?2, sold = sold + ?2 WHERE ticket_type_id = ?1",
        )
        .await
    }

    async fn return_sold(&mut self, items: &LineItems) -> StoreResult<()> {
        // `CHECK (sold >= 0)` refuses returning more than was sold.
        self.for_each_item(
            items,
            "UPDATE inventory SET sold = sold - ?2 WHERE ticket_type_id = ?1",
        )
        .await
    }

    async fn try_sell(&mut self, items: &LineItems) -> StoreResult<bool> {
        self.all_or_nothing(
            items,
            "UPDATE inventory SET sold = sold + ?2
             WHERE ticket_type_id = ?1 AND held + sold + ?2 <= capacity",
            "UPDATE inventory SET sold = sold - ?2 WHERE ticket_type_id = ?1",
            bind_item,
        )
        .await
    }

    async fn try_take_quota(&mut self, account: AccountId, holds: &Holds) -> StoreResult<bool> {
        for hold in holds {
            sqlx::query(
                "INSERT INTO quotas (account_id, ticket_type_id, taken) VALUES (?1, ?2, 0)
                 ON CONFLICT DO NOTHING",
            )
            .bind(account.as_uuid())
            .bind(hold.ticket_type_id.as_uuid())
            .execute(&mut *self.conn)
            .await
            .map_err(error)?;
        }
        let account = account.as_uuid();
        self.all_or_nothing(
            holds,
            "UPDATE quotas SET taken = taken + ?3
             WHERE account_id = ?1 AND ticket_type_id = ?2 AND taken + ?3 <= ?4",
            "UPDATE quotas SET taken = taken - ?3 WHERE account_id = ?1 AND ticket_type_id = ?2",
            move |query, hold: Hold| {
                query
                    .bind(account)
                    .bind(hold.ticket_type_id.as_uuid())
                    .bind(hold.quantity)
                    .bind(hold.per_account_limit)
            },
        )
        .await
    }

    async fn return_quota(&mut self, account: AccountId, items: &LineItems) -> StoreResult<()> {
        for item in items {
            sqlx::query(
                "UPDATE quotas SET taken = taken - ?3 WHERE account_id = ?1 AND ticket_type_id = ?2",
            )
            .bind(account.as_uuid())
            .bind(item.ticket_type_id.as_uuid())
            .bind(item.quantity)
            .execute(&mut *self.conn)
            .await
            .map_err(error)?;
        }
        Ok(())
    }
}

#[async_trait]
impl ReservationsTx for SqliteTx {
    async fn insert_reservation(&mut self, reservation: &Reservation) -> StoreResult<()> {
        sqlx::query(
            "INSERT INTO reservations (id, purchase_request_id, account_id, sale_id, event_id, items,
                                       total_minor, currency, environment, status, attestor_id,
                                       expires_at, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        )
        .bind(reservation.id.as_uuid())
        .bind(reservation.purchase_request_id.as_uuid())
        .bind(reservation.account_id.as_uuid())
        .bind(reservation.sale_id.as_uuid())
        .bind(reservation.event_id.as_uuid())
        .bind(json(&reservation.items)?)
        .bind(reservation.total.amount_minor())
        .bind(reservation.total.currency().as_str())
        .bind(reservation.environment.as_str())
        .bind(reservation.status.as_str())
        .bind(reservation.attestor_id.map(|id| id.as_uuid()))
        .bind(micros(reservation.expires_at))
        .bind(micros(reservation.created_at))
        .bind(micros(reservation.updated_at))
        .execute(&mut *self.conn)
        .await
        .map_err(unique("purchase_request_id"))?;
        Ok(())
    }

    async fn lock_reservation(&mut self, id: ReservationId) -> StoreResult<Option<Reservation>> {
        let row = sqlx::query_as::<_, ReservationRow>(sqlx::AssertSqlSafe(format!(
            "{RESERVATION_COLUMNS} WHERE id = ?1"
        )))
        .bind(id.as_uuid())
        .fetch_optional(&mut *self.conn)
        .await
        .map_err(error)?;
        optional(row)
    }

    async fn update_reservation(&mut self, reservation: &Reservation) -> StoreResult<()> {
        let result = sqlx::query(
            "UPDATE reservations SET status = ?2, attestor_id = ?3, expires_at = ?4, updated_at = ?5
             WHERE id = ?1",
        )
        .bind(reservation.id.as_uuid())
        .bind(reservation.status.as_str())
        .bind(reservation.attestor_id.map(|id| id.as_uuid()))
        .bind(micros(reservation.expires_at))
        .bind(micros(reservation.updated_at))
        .execute(&mut *self.conn)
        .await
        .map_err(error)?;
        if result.rows_affected() == 1 {
            Ok(())
        } else {
            Err(StoreError::NotFound("reservation"))
        }
    }
}
