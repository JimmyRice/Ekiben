use std::collections::HashMap;

use async_trait::async_trait;
use kippu_domain::purchase::{LineItem, PurchaseRequest, PurchaseStatus};
use kippu_domain::reservation::{Reservation, ReservationStatus};
use kippu_domain::{
    AccountId, EventId, OrganizationId, PurchaseRequestId, ReservationId, SaleId, TicketTypeId,
    Timestamp,
};
use kippu_store::{
    Holds, Insertion, InventoryTx, Keyset, LineItems, PageRequest, PurchaseStore, PurchasesTx,
    QueuedRequest, QuotaLines, ReservationsTx, StoreError, StoreResult,
};
use sqlx::MySqlConnection;
use uuid::Uuid;

use crate::convert::{
    PurchaseRequestRow, QueuedRequestRow, ReservationRow, all, json, micros, optional,
    purchase_status, uncount,
};
use crate::tx::MySqlTx;
use crate::{MySqlStore, error, is_duplicate, unique};

const PURCHASE_REQUEST_COLUMNS: &str = "id, account_id, sale_id, basket, status, reservation_id, rejection_reason, created_at, updated_at";

/// Whether a queued request's account (`r`) is on its sale's invitation list.
const INVITED: &str = "EXISTS (SELECT 1 FROM sale_invitations i \
     WHERE i.sale_id = r.sale_id AND i.account_id = r.account_id)";

/// What [`INVITED`] stands for when the list does not matter.
const EVERYONE_INVITED: &str = "TRUE";

const RESERVATION_COLUMNS: &str = "SELECT id, purchase_request_id, account_id, sale_id, event_id, \
     items, total_minor, currency, environment, status, attestor_id, expires_at, created_at, \
     updated_at FROM reservations";

/// The most rows one multi-row statement carries, keeping its placeholders far below MySQL's
/// limit of 65,535 for every table written this way.
const ROWS_PER_STATEMENT: usize = 1_000;

/// `count` copies of `row`, comma-separated: the rows of a multi-row statement.
fn rows(row: &str, count: usize) -> String {
    vec![row; count].join(", ")
}

/// Records a purchase request in its status, unless one with its id exists: then returns that.
/// A duplicate key undoes the one statement, not the transaction around it, which can go on.
async fn insert_request(
    conn: &mut MySqlConnection,
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
    .execute(&mut *conn)
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
    .fetch_one(&mut *conn)
    .await
    .map_err(error)?;
    Ok(Insertion::Existing(existing.try_into()?))
}

#[async_trait]
impl PurchaseStore for MySqlStore {
    async fn insert_purchase_request(
        &self,
        request: &PurchaseRequest,
    ) -> StoreResult<Insertion<PurchaseRequest>> {
        let mut conn = self.pool.acquire().await.map_err(error)?;
        insert_request(&mut conn, request).await
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

    async fn sales_with_queued_requests(
        &self,
        now: Timestamp,
        limit: u32,
    ) -> StoreResult<Vec<(SaleId, u32)>> {
        let sales = sqlx::query_as::<_, (Uuid, i64)>(
            "SELECT sale_id, COUNT(*) FROM (SELECT sale_id, created_at FROM purchase_requests
                                  WHERE status = 'queued'
                                    AND (lease_until IS NULL OR lease_until <= ?)
                                  ORDER BY created_at LIMIT ?) AS oldest
             GROUP BY sale_id ORDER BY MIN(created_at), sale_id",
        )
        .bind(micros(now))
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(error)?;
        sales
            .into_iter()
            .map(|(sale, queued)| {
                Ok((
                    sale.into(),
                    u32::try_from(queued).map_err(StoreError::backend)?,
                ))
            })
            .collect()
    }

    async fn postpone_purchase_request(
        &self,
        id: PurchaseRequestId,
        until: Timestamp,
    ) -> StoreResult<()> {
        sqlx::query(
            "UPDATE purchase_requests SET lease_until = ? WHERE id = ? AND status = 'queued'",
        )
        .bind(micros(until))
        .bind(id.as_uuid())
        .execute(&self.pool)
        .await
        .map_err(error)?;
        Ok(())
    }

    async fn queued_purchase_count(&self, sale: SaleId, cap: u32) -> StoreResult<u64> {
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM (SELECT 1 FROM purchase_requests
                                   WHERE sale_id = ? AND status = 'queued' LIMIT ?) AS queued",
        )
        .bind(sale.as_uuid())
        .bind(i64::from(cap))
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

    async fn reservations_for_event(
        &self,
        event: EventId,
        status: Option<ReservationStatus>,
        page: PageRequest<Keyset>,
    ) -> StoreResult<Vec<Reservation>> {
        let status = status.map(ReservationStatus::as_str);
        let after_at = page.after.map(|after| micros(after.at));
        let rows = sqlx::query_as::<_, ReservationRow>(sqlx::AssertSqlSafe(format!(
            "{RESERVATION_COLUMNS} WHERE event_id = ? AND (? IS NULL OR status = ?)
               AND (? IS NULL OR created_at < ? OR (created_at = ? AND id > ?))
             ORDER BY created_at DESC, id LIMIT ?"
        )))
        .bind(event.as_uuid())
        .bind(status)
        .bind(status)
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

    async fn has_orders(&self, event: EventId, account: Option<AccountId>) -> StoreResult<bool> {
        let account = account.map(|id| id.as_uuid());
        let found = sqlx::query_scalar::<_, i64>(
            "SELECT EXISTS (SELECT 1 FROM reservations
                            WHERE event_id = ? AND (? IS NULL OR account_id = ?)
                              AND status NOT IN ('expired', 'cancelled'))",
        )
        .bind(event.as_uuid())
        .bind(account)
        .bind(account)
        .fetch_one(&self.pool)
        .await
        .map_err(error)?;
        Ok(found != 0)
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
    async fn lock_purchase_queue(&mut self, sale: SaleId) -> StoreResult<bool> {
        let taken = sqlx::query_scalar::<_, i64>(
            "SELECT 1 FROM purchase_queues WHERE sale_id = ? FOR UPDATE SKIP LOCKED",
        )
        .bind(sale.as_uuid())
        .fetch_optional(&mut *self.conn)
        .await
        .map_err(error)?;
        if taken.is_some() {
            return Ok(true);
        }
        // Skipped because another transaction holds it, or missing because an instance older
        // than the queues created the sale. An insert would wait on a held row, so a plain
        // (unlocked) read tells the two apart first.
        let exists =
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM purchase_queues WHERE sale_id = ?")
                .bind(sale.as_uuid())
                .fetch_one(&mut *self.conn)
                .await
                .map_err(error)?;
        if exists > 0 {
            return Ok(false);
        }
        let created = sqlx::query("INSERT INTO purchase_queues (sale_id) VALUES (?)")
            .bind(sale.as_uuid())
            .execute(&mut *self.conn)
            .await;
        match created {
            Ok(_) => Ok(true),
            Err(failure) if is_duplicate(&failure) => Ok(false),
            Err(failure) => Err(error(failure)),
        }
    }

    async fn insert_purchase_request(
        &mut self,
        request: &PurchaseRequest,
    ) -> StoreResult<Insertion<PurchaseRequest>> {
        insert_request(&mut self.conn, request).await
    }

    async fn queued_requests(
        &mut self,
        sale: SaleId,
        event: EventId,
        organization: OrganizationId,
        invited_only: bool,
        now: Timestamp,
        limit: u32,
    ) -> StoreResult<Vec<QueuedRequest>> {
        let invited = if invited_only {
            INVITED
        } else {
            EVERYONE_INVITED
        };
        // The sale's queue index must drive the scan: with a filesort MySQL would lock every
        // queued request of the sale before applying LIMIT. FOR UPDATE: a worker of the
        // previous release, which leases requests instead of holding the queue, may be serving
        // some; they are left out once it commits.
        let rows = sqlx::query_as::<_, QueuedRequestRow>(sqlx::AssertSqlSafe(format!(
            "SELECT {PURCHASE_REQUEST_COLUMNS},
                    EXISTS (SELECT 1 FROM denials d
                            WHERE d.subject_id = r.account_id AND d.subject_kind = 'account'
                              AND d.organization_id = ?
                              AND (d.event_id IS NULL OR d.event_id = ?)) AS account_denied,
                    {invited} AS account_invited
             FROM purchase_requests r FORCE INDEX (purchase_requests_sale_queue)
             WHERE r.sale_id = ? AND r.status = 'queued'
               AND (r.lease_until IS NULL OR r.lease_until <= ?)
             ORDER BY r.created_at, r.id
             LIMIT ?
             FOR UPDATE OF r"
        )))
        .bind(organization.as_uuid())
        .bind(event.as_uuid())
        .bind(sale.as_uuid())
        .bind(micros(now))
        .bind(limit)
        .fetch_all(&mut *self.conn)
        .await
        .map_err(error)?;
        all(rows)
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

    async fn set_purchase_statuses(
        &mut self,
        outcomes: &[(PurchaseRequestId, PurchaseStatus)],
        now: Timestamp,
    ) -> StoreResult<()> {
        for chunk in outcomes.chunks(ROWS_PER_STATEMENT) {
            let statement = format!(
                "UPDATE purchase_requests r
                 JOIN (VALUES {}) AS o (id, status, reservation_id, rejection_reason)
                   ON r.id = o.id
                 SET r.status = o.status, r.reservation_id = o.reservation_id,
                     r.rejection_reason = o.rejection_reason, r.lease_until = NULL,
                     r.updated_at = ?",
                // Parameters in a table value constructor are typed as text; ids are bytes.
                rows(
                    "ROW(CAST(? AS BINARY(16)), ?, CAST(? AS BINARY(16)), ?)",
                    chunk.len()
                )
            );
            let mut query = sqlx::query(sqlx::AssertSqlSafe(statement));
            for &(id, status) in chunk {
                let columns = purchase_status(status);
                query = query
                    .bind(id.as_uuid())
                    .bind(columns.status)
                    .bind(columns.reservation_id)
                    .bind(columns.rejection_reason);
            }
            query
                .bind(micros(now))
                .execute(&mut *self.conn)
                .await
                .map_err(error)?;
        }
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
    async fn lock_stock(&mut self, items: &LineItems) -> StoreResult<Vec<u32>> {
        // One row per statement, so the locks are taken exactly in the items' order.
        let mut available = Vec::with_capacity(items.len());
        for item in items {
            let left = sqlx::query_scalar::<_, i64>(
                "SELECT capacity - held - sold FROM inventory WHERE ticket_type_id = ? FOR UPDATE",
            )
            .bind(item.ticket_type_id.as_uuid())
            .fetch_optional(&mut *self.conn)
            .await
            .map_err(error)?;
            available.push(left.map_or(Ok(0), uncount)?);
        }
        Ok(available)
    }

    async fn try_hold(&mut self, items: &LineItems) -> StoreResult<bool> {
        self.stock_all_or_nothing(items, &HOLD).await
    }

    async fn release_held(&mut self, items: &LineItems) -> StoreResult<()> {
        for item in items {
            self.stock(HOLD.undo, item).await?;
        }
        Ok(())
    }

    async fn sell_held(&mut self, items: &LineItems) -> StoreResult<()> {
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

    async fn return_sold(&mut self, items: &LineItems) -> StoreResult<()> {
        // Undoing a sale. `CHECK (sold >= 0)` refuses returning more than was sold: the
        // quantity is signed, so the subtraction goes negative instead of wrapping.
        for item in items {
            self.stock(SELL.undo, item).await?;
        }
        Ok(())
    }

    async fn try_sell(&mut self, items: &LineItems) -> StoreResult<bool> {
        self.stock_all_or_nothing(items, &SELL).await
    }

    async fn try_take_quota(&mut self, account: AccountId, holds: &Holds) -> StoreResult<bool> {
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

    async fn return_quota(&mut self, account: AccountId, items: &LineItems) -> StoreResult<()> {
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

    async fn lock_quotas(&mut self, lines: &QuotaLines) -> StoreResult<Vec<u32>> {
        let mut taken = HashMap::with_capacity(lines.len());
        for chunk in lines.chunks(ROWS_PER_STATEMENT) {
            // Inserting the quotas never taken and touching the others locks them all, in the
            // lines' order (chunks follow it too).
            let mut insert = sqlx::query(sqlx::AssertSqlSafe(format!(
                "INSERT INTO quotas (account_id, ticket_type_id, taken) VALUES {}
                 ON DUPLICATE KEY UPDATE taken = taken",
                rows("(?, ?, 0)", chunk.len())
            )));
            for line in chunk {
                insert = insert
                    .bind(line.account_id.as_uuid())
                    .bind(line.ticket_type_id.as_uuid());
            }
            insert.execute(&mut *self.conn).await.map_err(error)?;
            // Locked by this transaction, the rows cannot change but through it: a plain read
            // sees what a locking read would.
            let mut select = sqlx::query_as::<_, (Uuid, Uuid, i64)>(sqlx::AssertSqlSafe(format!(
                "SELECT account_id, ticket_type_id, taken FROM quotas
                 WHERE (account_id, ticket_type_id) IN ({})",
                rows("(?, ?)", chunk.len())
            )));
            for line in chunk {
                select = select
                    .bind(line.account_id.as_uuid())
                    .bind(line.ticket_type_id.as_uuid());
            }
            for (account, ticket_type, count) in
                select.fetch_all(&mut *self.conn).await.map_err(error)?
            {
                taken.insert((account, ticket_type), count);
            }
        }
        lines
            .iter()
            .map(|line| {
                taken
                    .get(&(line.account_id.as_uuid(), line.ticket_type_id.as_uuid()))
                    .copied()
                    .ok_or(StoreError::NotFound("quota"))
                    .and_then(uncount)
            })
            .collect()
    }

    async fn take_quotas(&mut self, lines: &QuotaLines) -> StoreResult<()> {
        for chunk in lines.chunks(ROWS_PER_STATEMENT) {
            let mut query = sqlx::query(sqlx::AssertSqlSafe(format!(
                "INSERT INTO quotas (account_id, ticket_type_id, taken) VALUES {} AS added
                 ON DUPLICATE KEY UPDATE taken = quotas.taken + added.taken",
                rows("(?, ?, ?)", chunk.len())
            )));
            for line in chunk {
                query = query
                    .bind(line.account_id.as_uuid())
                    .bind(line.ticket_type_id.as_uuid())
                    .bind(i64::from(line.quantity));
            }
            query.execute(&mut *self.conn).await.map_err(error)?;
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

    async fn insert_reservations(&mut self, reservations: &[Reservation]) -> StoreResult<()> {
        for chunk in reservations.chunks(ROWS_PER_STATEMENT) {
            let mut query = sqlx::query(sqlx::AssertSqlSafe(format!(
                "INSERT INTO reservations (id, purchase_request_id, account_id, sale_id, event_id,
                                           items, total_minor, currency, environment, status,
                                           attestor_id, expires_at, created_at, updated_at)
                 VALUES {}",
                rows("(?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)", chunk.len())
            )));
            for reservation in chunk {
                query = query
                    .bind(reservation.id.as_uuid())
                    .bind(reservation.purchase_request_id.as_uuid())
                    .bind(reservation.account_id.as_uuid())
                    .bind(reservation.sale_id.as_uuid())
                    .bind(reservation.event_id.as_uuid())
                    .bind(json(&reservation.items)?)
                    .bind(reservation.total.amount_minor())
                    .bind(reservation.total.currency().as_str().to_owned())
                    .bind(reservation.environment.as_str())
                    .bind(reservation.status.as_str())
                    .bind(reservation.attestor_id.map(|id| id.as_uuid()))
                    .bind(micros(reservation.expires_at))
                    .bind(micros(reservation.created_at))
                    .bind(micros(reservation.updated_at));
            }
            query
                .execute(&mut *self.conn)
                .await
                .map_err(unique("purchase_request_id"))?;
        }
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
