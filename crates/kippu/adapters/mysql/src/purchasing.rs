use async_trait::async_trait;
use kippu_domain::purchase::{LineItem, PurchaseRequest, PurchaseStatus};
use kippu_domain::reservation::Reservation;
use kippu_domain::{AccountId, PurchaseRequestId, ReservationId, SaleId, TicketTypeId, Timestamp};
use kippu_store::{
    Hold, Insertion, InventoryTx, Keyset, Lease, PageRequest, PurchaseStore, PurchasesTx,
    ReservationsTx, StoreError, StoreResult,
};
use uuid::Uuid;

use crate::convert::{
    PurchaseRequestRow, ReservationRow, all, json, micros, optional, purchase_status,
};
use crate::tx::MySqlTx;
use crate::{MySqlStore, error, is_duplicate, unique};

const PURCHASE_REQUEST_COLUMNS: &str = "id, account_id, sale_id, basket, status, reservation_id, rejection_reason, created_at, updated_at";

const RESERVATION_COLUMNS: &str = "SELECT id, purchase_request_id, account_id, sale_id, event_id, \
     items, total_minor, currency, environment, status, attestor_id, expires_at, created_at, \
     updated_at FROM reservations";

#[async_trait]
impl PurchaseStore for MySqlStore {
    async fn insert_purchase_request(
        &self,
        request: &PurchaseRequest,
    ) -> StoreResult<Insertion<PurchaseRequest>> {
        let status = purchase_status(request.status);
        let inserted = sqlx::query(
            "INSERT INTO purchase_requests (id, account_id, sale_id, basket, status, reservation_id,
                                            rejection_reason, lease_until, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, NULL, ?, ?)",
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
        .execute(&self.pool)
        .await;
        match inserted {
            Ok(_) => return Ok(Insertion::Inserted),
            Err(failure) if is_duplicate(&failure) => {}
            Err(failure) => return Err(error(failure)),
        }
        let existing = sqlx::query_as::<_, PurchaseRequestRow>(sqlx::AssertSqlSafe(format!(
            "SELECT {PURCHASE_REQUEST_COLUMNS} FROM purchase_requests WHERE id = ?"
        )))
        .bind(request.id.as_uuid())
        .fetch_one(&self.pool)
        .await
        .map_err(error)?;
        Ok(Insertion::Existing(existing.try_into()?))
    }

    async fn purchase_request(
        &self,
        id: PurchaseRequestId,
    ) -> StoreResult<Option<PurchaseRequest>> {
        let row = sqlx::query_as::<_, PurchaseRequestRow>(sqlx::AssertSqlSafe(format!(
            "SELECT {PURCHASE_REQUEST_COLUMNS} FROM purchase_requests WHERE id = ?"
        )))
        .bind(id.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(error)?;
        optional(row)
    }

    async fn claim_purchase_requests(
        &self,
        lease: Lease,
        limit: u32,
    ) -> StoreResult<Vec<PurchaseRequest>> {
        // No UPDATE … RETURNING in MySQL: lock the batch, lease it, read it, in one transaction.
        // The queue index must drive the scan: with a filesort MySQL reads (and locks) every
        // queued row before applying LIMIT, and concurrent claimers would skip them all.
        let mut tx = self.pool.begin().await.map_err(error)?;
        let ids = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM purchase_requests FORCE INDEX (purchase_requests_queue)
             WHERE status = 'queued' AND (lease_until IS NULL OR lease_until <= ?)
             ORDER BY created_at LIMIT ?
             FOR UPDATE SKIP LOCKED",
        )
        .bind(micros(lease.now))
        .bind(limit)
        .fetch_all(&mut *tx)
        .await
        .map_err(error)?;
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = vec!["?"; ids.len()].join(", ");
        let mut update = sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE purchase_requests SET lease_until = ? WHERE id IN ({placeholders})"
        )))
        .bind(micros(lease.until));
        for id in &ids {
            update = update.bind(id);
        }
        update.execute(&mut *tx).await.map_err(error)?;
        let mut select = sqlx::query_as::<_, PurchaseRequestRow>(sqlx::AssertSqlSafe(format!(
            "SELECT {PURCHASE_REQUEST_COLUMNS} FROM purchase_requests WHERE id IN ({placeholders})"
        )));
        for id in &ids {
            select = select.bind(id);
        }
        let rows = select.fetch_all(&mut *tx).await.map_err(error)?;
        tx.commit().await.map_err(error)?;
        let mut requests: Vec<PurchaseRequest> = all(rows)?;
        requests.sort_by_key(|request| request.created_at);
        Ok(requests)
    }

    async fn queued_purchase_count(&self, sale: SaleId) -> StoreResult<u64> {
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM purchase_requests WHERE sale_id = ? AND status = 'queued'",
        )
        .bind(sale.as_uuid())
        .fetch_one(&self.pool)
        .await
        .map_err(error)?;
        u64::try_from(count).map_err(StoreError::backend)
    }

    async fn reservation(&self, id: ReservationId) -> StoreResult<Option<Reservation>> {
        let row = sqlx::query_as::<_, ReservationRow>(sqlx::AssertSqlSafe(format!(
            "{RESERVATION_COLUMNS} WHERE id = ?"
        )))
        .bind(id.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(error)?;
        optional(row)
    }

    async fn reservations_for_account(
        &self,
        account: AccountId,
        page: PageRequest<Keyset>,
    ) -> StoreResult<Vec<Reservation>> {
        let after_at = page.after.map(|after| micros(after.at));
        let rows = sqlx::query_as::<_, ReservationRow>(sqlx::AssertSqlSafe(format!(
            "{RESERVATION_COLUMNS} WHERE account_id = ?
               AND (? IS NULL OR created_at < ? OR (created_at = ? AND id > ?))
             ORDER BY created_at DESC, id LIMIT ?"
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

    async fn overdue_reservations(
        &self,
        now: Timestamp,
        limit: u32,
    ) -> StoreResult<Vec<ReservationId>> {
        let ids = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM reservations
             WHERE status IN ('reserved', 'payment_pending') AND expires_at <= ?
             ORDER BY expires_at LIMIT ?",
        )
        .bind(micros(now))
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(error)?;
        Ok(ids.into_iter().map(Into::into).collect())
    }
}

#[async_trait]
impl PurchasesTx for MySqlTx {
    async fn lock_purchase_request(
        &mut self,
        id: PurchaseRequestId,
    ) -> StoreResult<Option<PurchaseRequest>> {
        let row = sqlx::query_as::<_, PurchaseRequestRow>(sqlx::AssertSqlSafe(format!(
            "SELECT {PURCHASE_REQUEST_COLUMNS} FROM purchase_requests WHERE id = ? FOR UPDATE"
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
             SET status = ?, reservation_id = ?, rejection_reason = ?, lease_until = NULL,
                 updated_at = ?
             WHERE id = ?",
        )
        .bind(columns.status)
        .bind(columns.reservation_id)
        .bind(columns.rejection_reason)
        .bind(micros(now))
        .bind(id.as_uuid())
        .execute(&mut *self.conn)
        .await
        .map_err(error)?;
        Ok(())
    }
}

/// A stock or quota change: `apply` must affect exactly one row for the change to count, and
/// `undo` reverts it. Both take `(quantity, ticket type)` and, for quotas, more parameters
/// after those — see each call.
struct Change {
    apply: &'static str,
    undo: &'static str,
}

const HOLD: Change = Change {
    apply: "UPDATE inventory SET held = held + ?
            WHERE ticket_type_id = ? AND held + sold + ? <= capacity",
    undo: "UPDATE inventory SET held = held - ? WHERE ticket_type_id = ?",
};

const SELL: Change = Change {
    apply: "UPDATE inventory SET sold = sold + ?
            WHERE ticket_type_id = ? AND held + sold + ? <= capacity",
    undo: "UPDATE inventory SET sold = sold - ? WHERE ticket_type_id = ?",
};

impl MySqlTx {
    /// Applies `change` to every item. If one has no stock, reverts the ones before it and
    /// returns `false`, so the operation as a whole is all-or-nothing. Quantities are bound as
    /// signed integers: MySQL arithmetic with an unsigned operand cannot go below zero.
    async fn stock_all_or_nothing(
        &mut self,
        items: &[LineItem],
        change: &Change,
    ) -> StoreResult<bool> {
        for (applied, item) in items.iter().enumerate() {
            let quantity = i64::from(item.quantity);
            let result = sqlx::query(change.apply)
                .bind(quantity)
                .bind(item.ticket_type_id.as_uuid())
                .bind(quantity)
                .execute(&mut *self.conn)
                .await
                .map_err(error)?;
            if result.rows_affected() == 0 {
                for done in items.iter().take(applied) {
                    self.stock(change.undo, done).await?;
                }
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Runs `statement`, taking `(quantity, ticket type)`, for one item.
    async fn stock(&mut self, statement: &'static str, item: &LineItem) -> StoreResult<()> {
        sqlx::query(statement)
            .bind(i64::from(item.quantity))
            .bind(item.ticket_type_id.as_uuid())
            .execute(&mut *self.conn)
            .await
            .map_err(error)?;
        Ok(())
    }

    /// Changes an account's quota by `delta`; `limit` (when given) must not be exceeded.
    async fn quota(
        &mut self,
        account: AccountId,
        ticket_type: TicketTypeId,
        delta: i64,
        limit: Option<u32>,
    ) -> StoreResult<bool> {
        let result = match limit {
            Some(limit) => sqlx::query(
                "UPDATE quotas SET taken = taken + ?
                 WHERE account_id = ? AND ticket_type_id = ? AND taken + ? <= ?",
            )
            .bind(delta)
            .bind(account.as_uuid())
            .bind(ticket_type.as_uuid())
            .bind(delta)
            .bind(i64::from(limit)),
            None => sqlx::query(
                "UPDATE quotas SET taken = taken + ? WHERE account_id = ? AND ticket_type_id = ?",
            )
            .bind(delta)
            .bind(account.as_uuid())
            .bind(ticket_type.as_uuid()),
        }
        .execute(&mut *self.conn)
        .await
        .map_err(error)?;
        Ok(result.rows_affected() == 1)
    }
}

#[async_trait]
impl InventoryTx for MySqlTx {
    async fn try_hold(&mut self, items: &[LineItem]) -> StoreResult<bool> {
        self.stock_all_or_nothing(items, &HOLD).await
    }

    async fn release_held(&mut self, items: &[LineItem]) -> StoreResult<()> {
        for item in items {
            self.stock(HOLD.undo, item).await?;
        }
        Ok(())
    }

    async fn sell_held(&mut self, items: &[LineItem]) -> StoreResult<()> {
        for item in items {
            sqlx::query(
                "UPDATE inventory SET held = held - ?, sold = sold + ? WHERE ticket_type_id = ?",
            )
            .bind(i64::from(item.quantity))
            .bind(i64::from(item.quantity))
            .bind(item.ticket_type_id.as_uuid())
            .execute(&mut *self.conn)
            .await
            .map_err(error)?;
        }
        Ok(())
    }

    async fn return_sold(&mut self, items: &[LineItem]) -> StoreResult<()> {
        // Undoing a sale. `CHECK (sold >= 0)` refuses returning more than was sold: the
        // quantity is signed, so the subtraction goes negative instead of wrapping.
        for item in items {
            self.stock(SELL.undo, item).await?;
        }
        Ok(())
    }

    async fn try_sell(&mut self, items: &[LineItem]) -> StoreResult<bool> {
        self.stock_all_or_nothing(items, &SELL).await
    }

    async fn try_take_quota(&mut self, account: AccountId, holds: &[Hold]) -> StoreResult<bool> {
        for hold in holds {
            sqlx::query(
                "INSERT INTO quotas (account_id, ticket_type_id, taken) VALUES (?, ?, 0)
                 ON DUPLICATE KEY UPDATE taken = taken",
            )
            .bind(account.as_uuid())
            .bind(hold.ticket_type_id.as_uuid())
            .execute(&mut *self.conn)
            .await
            .map_err(error)?;
        }
        for (applied, hold) in holds.iter().enumerate() {
            let quantity = i64::from(hold.quantity);
            let taken = self
                .quota(
                    account,
                    hold.ticket_type_id,
                    quantity,
                    Some(hold.per_account_limit),
                )
                .await?;
            if !taken {
                for done in holds.iter().take(applied) {
                    self.quota(
                        account,
                        done.ticket_type_id,
                        -i64::from(done.quantity),
                        None,
                    )
                    .await?;
                }
                return Ok(false);
            }
        }
        Ok(true)
    }

    async fn return_quota(&mut self, account: AccountId, items: &[LineItem]) -> StoreResult<()> {
        for item in items {
            self.quota(
                account,
                item.ticket_type_id,
                -i64::from(item.quantity),
                None,
            )
            .await?;
        }
        Ok(())
    }
}

#[async_trait]
impl ReservationsTx for MySqlTx {
    async fn insert_reservation(&mut self, reservation: &Reservation) -> StoreResult<()> {
        sqlx::query(
            "INSERT INTO reservations (id, purchase_request_id, account_id, sale_id, event_id, items,
                                       total_minor, currency, environment, status, attestor_id,
                                       expires_at, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(reservation.id.as_uuid())
        .bind(reservation.purchase_request_id.as_uuid())
        .bind(reservation.account_id.as_uuid())
        .bind(reservation.sale_id.as_uuid())
        .bind(reservation.event_id.as_uuid())
        .bind(json(&reservation.items)?)
        .bind(reservation.total.amount_minor)
        .bind(reservation.total.currency.as_str())
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
            "{RESERVATION_COLUMNS} WHERE id = ? FOR UPDATE"
        )))
        .bind(id.as_uuid())
        .fetch_optional(&mut *self.conn)
        .await
        .map_err(error)?;
        optional(row)
    }

    async fn update_reservation(&mut self, reservation: &Reservation) -> StoreResult<()> {
        let result = sqlx::query(
            "UPDATE reservations SET status = ?, attestor_id = ?, expires_at = ?, updated_at = ?
             WHERE id = ?",
        )
        .bind(reservation.status.as_str())
        .bind(reservation.attestor_id.map(|id| id.as_uuid()))
        .bind(micros(reservation.expires_at))
        .bind(micros(reservation.updated_at))
        .bind(reservation.id.as_uuid())
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
