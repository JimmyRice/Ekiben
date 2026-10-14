use std::collections::HashMap;

use async_trait::async_trait;
use kippu_domain::purchase::{LineItem, PurchaseRequest, PurchaseStatus};
use kippu_domain::reservation::{Reservation, ReservationStatus, ReservedItem};
use kippu_domain::{
    AccountId, EventId, OrganizationId, PurchaseRequestId, ReservationId, SaleId, Timestamp,
};
use kippu_store::{
    Hold, Holds, Insertion, InventoryTx, Keyset, LineItems, PageRequest, PurchaseStore,
    PurchasesTx, QueuedRequest, QuotaLines, ReservationsTx, StoreError, StoreResult,
};
use sqlx::types::Json;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::convert::{
    PurchaseRequestRow, QueuedRequestRow, ReservationRow, all, at, count, optional,
    purchase_status, uncount,
};
use crate::tx::PostgresTx;
use crate::{PostgresStore, error, unique};

/// A query whose parameters are still being bound.
type PgQuery<'q> = sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>;

const PURCHASE_REQUEST_COLUMNS: &str = "id, account_id, sale_id, basket, status, reservation_id, rejection_reason, created_at, updated_at";

const RESERVATION_COLUMNS: &str = "SELECT id, purchase_request_id, account_id, sale_id, event_id, \
     items, total_minor, currency, environment, status, attestor_id, expires_at, created_at, \
     updated_at FROM reservations";

#[async_trait]
impl PurchaseStore for PostgresStore {
    async fn insert_purchase_request(
        &self,
        request: &PurchaseRequest,
    ) -> StoreResult<Insertion<PurchaseRequest>> {
        let status = purchase_status(request.status);
        let inserted = sqlx::query(
            "INSERT INTO purchase_requests (id, account_id, sale_id, basket, status, reservation_id,
                                            rejection_reason, lease_until, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, NULL, $8, $9)
             ON CONFLICT (id) DO NOTHING",
        )
        .bind(request.id.as_uuid())
        .bind(request.account_id.as_uuid())
        .bind(request.sale_id.as_uuid())
        .bind(Json(&request.basket))
        .bind(status.status)
        .bind(status.reservation_id)
        .bind(status.rejection_reason)
        .bind(at(request.created_at))
        .bind(at(request.updated_at))
        .execute(&self.pool)
        .await
        .map_err(error)?;
        if inserted.rows_affected() == 1 {
            return Ok(Insertion::Inserted);
        }
        let existing = sqlx::query_as::<_, PurchaseRequestRow>(sqlx::AssertSqlSafe(format!(
            "SELECT {PURCHASE_REQUEST_COLUMNS} FROM purchase_requests WHERE id = $1"
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
            "SELECT {PURCHASE_REQUEST_COLUMNS} FROM purchase_requests WHERE id = $1"
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
                                    AND (lease_until IS NULL OR lease_until <= $1)
                                  ORDER BY created_at LIMIT $2) AS oldest
             GROUP BY sale_id ORDER BY MIN(created_at), sale_id",
        )
        .bind(at(now))
        .bind(i64::from(limit))
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
            "UPDATE purchase_requests SET lease_until = $2 WHERE id = $1 AND status = 'queued'",
        )
        .bind(id.as_uuid())
        .bind(at(until))
        .execute(&self.pool)
        .await
        .map_err(error)?;
        Ok(())
    }

    async fn queued_purchase_count(&self, sale: SaleId, cap: u32) -> StoreResult<u64> {
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM (SELECT 1 FROM purchase_requests
                                   WHERE sale_id = $1 AND status = 'queued' LIMIT $2) AS queued",
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
            "{RESERVATION_COLUMNS} WHERE id = $1"
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
        let rows = sqlx::query_as::<_, ReservationRow>(sqlx::AssertSqlSafe(format!(
            "{RESERVATION_COLUMNS} WHERE account_id = $1
               AND ($2 IS NULL OR created_at < $2 OR (created_at = $2 AND id > $3))
             ORDER BY created_at DESC, id LIMIT $4"
        )))
        .bind(account.as_uuid())
        .bind(page.after.map(|after| at(after.at)))
        .bind(page.after.map(|after| after.id))
        .bind(i64::from(page.limit))
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
        let rows = sqlx::query_as::<_, ReservationRow>(sqlx::AssertSqlSafe(format!(
            "{RESERVATION_COLUMNS} WHERE event_id = $1 AND ($2 IS NULL OR status = $2)
               AND ($3 IS NULL OR created_at < $3 OR (created_at = $3 AND id > $4))
             ORDER BY created_at DESC, id LIMIT $5"
        )))
        .bind(event.as_uuid())
        .bind(status.map(ReservationStatus::as_str))
        .bind(page.after.map(|after| at(after.at)))
        .bind(page.after.map(|after| after.id))
        .bind(i64::from(page.limit))
        .fetch_all(&self.pool)
        .await
        .map_err(error)?;
        all(rows)
    }

    async fn has_orders(&self, event: EventId, account: Option<AccountId>) -> StoreResult<bool> {
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM reservations
                            WHERE event_id = $1 AND ($2 IS NULL OR account_id = $2)
                              AND status NOT IN ('expired', 'cancelled'))",
        )
        .bind(event.as_uuid())
        .bind(account.map(|id| id.as_uuid()))
        .fetch_one(&self.pool)
        .await
        .map_err(error)
    }

    async fn overdue_reservations(
        &self,
        now: Timestamp,
        limit: u32,
    ) -> StoreResult<Vec<ReservationId>> {
        let ids = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM reservations
             WHERE status IN ('reserved', 'payment_pending') AND expires_at <= $1
             ORDER BY expires_at LIMIT $2",
        )
        .bind(at(now))
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(error)?;
        Ok(ids.into_iter().map(Into::into).collect())
    }
}

#[async_trait]
impl PurchasesTx for PostgresTx {
    async fn lock_purchase_queue(&mut self, sale: SaleId) -> StoreResult<bool> {
        // A worker sends its statements back to back. One whose machine froze or vanished
        // without closing the connection would hold the queue until TCP gave up on it (hours by
        // default), so a transaction holding a queue ends after 10 seconds without a statement.
        let taken = sqlx::query_scalar::<_, String>(
            "SELECT set_config('idle_in_transaction_session_timeout', '10s', true)
             FROM purchase_queues WHERE sale_id = $1 FOR UPDATE SKIP LOCKED",
        )
        .bind(sale.as_uuid())
        .fetch_optional(&mut *self.conn)
        .await
        .map_err(error)?;
        if taken.is_some() {
            return Ok(true);
        }
        // Skipped because another transaction holds it, or missing because an instance older
        // than the queues created the sale. Creating the row takes it; an existing one, held
        // or not, makes this insert do nothing without waiting.
        let created = sqlx::query_scalar::<_, String>(
            "INSERT INTO purchase_queues (sale_id) VALUES ($1) ON CONFLICT (sale_id) DO NOTHING
             RETURNING set_config('idle_in_transaction_session_timeout', '10s', true)",
        )
        .bind(sale.as_uuid())
        .fetch_optional(&mut *self.conn)
        .await
        .map_err(error)?;
        Ok(created.is_some())
    }

    async fn queued_requests(
        &mut self,
        sale: SaleId,
        event: EventId,
        organization: OrganizationId,
        now: Timestamp,
        limit: u32,
    ) -> StoreResult<Vec<QueuedRequest>> {
        // FOR UPDATE: a worker of the previous release, which leases requests instead of
        // holding the queue, may be serving some; they are left out once it commits.
        let rows = sqlx::query_as::<_, QueuedRequestRow>(sqlx::AssertSqlSafe(format!(
            "SELECT {PURCHASE_REQUEST_COLUMNS},
                    EXISTS (SELECT 1 FROM denials d
                            WHERE d.subject_id = r.account_id AND d.subject_kind = 'account'
                              AND d.organization_id = $2
                              AND (d.event_id IS NULL OR d.event_id = $3)) AS account_denied
             FROM purchase_requests r
             WHERE r.sale_id = $1 AND r.status = 'queued'
               AND (r.lease_until IS NULL OR r.lease_until <= $4)
             ORDER BY r.created_at, r.id
             LIMIT $5
             FOR UPDATE OF r"
        )))
        .bind(sale.as_uuid())
        .bind(organization.as_uuid())
        .bind(event.as_uuid())
        .bind(at(now))
        .bind(i64::from(limit))
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
             SET status = $2, reservation_id = $3, rejection_reason = $4, lease_until = NULL,
                 updated_at = $5
             WHERE id = $1",
        )
        .bind(id.as_uuid())
        .bind(columns.status)
        .bind(columns.reservation_id)
        .bind(columns.rejection_reason)
        .bind(at(now))
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
        let mut ids = Vec::with_capacity(outcomes.len());
        let mut statuses = Vec::with_capacity(outcomes.len());
        let mut reservations = Vec::with_capacity(outcomes.len());
        let mut reasons = Vec::with_capacity(outcomes.len());
        for &(id, status) in outcomes {
            let columns = purchase_status(status);
            ids.push(id.as_uuid());
            statuses.push(columns.status);
            reservations.push(columns.reservation_id);
            reasons.push(columns.rejection_reason);
        }
        sqlx::query(
            "UPDATE purchase_requests r
             SET status = o.status, reservation_id = o.reservation_id,
                 rejection_reason = o.rejection_reason, lease_until = NULL, updated_at = $5
             FROM unnest($1::uuid[], $2::text[], $3::uuid[], $4::text[])
                  AS o (id, status, reservation_id, rejection_reason)
             WHERE r.id = o.id",
        )
        .bind(ids)
        .bind(statuses)
        .bind(reservations)
        .bind(reasons)
        .bind(at(now))
        .execute(&mut *self.conn)
        .await
        .map_err(error)?;
        Ok(())
    }
}

impl PostgresTx {
    /// Runs a conditional update per item. If any affects no row, reverts the ones that did with
    /// `undo` and returns `false`, so the operation as a whole is all-or-nothing.
    async fn all_or_nothing<T: Copy + Send + Sync>(
        &mut self,
        items: &[T],
        apply: &'static str,
        undo: &'static str,
        bind: impl Fn(PgQuery<'_>, T) -> PgQuery<'_> + Send + Sync,
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
                .bind(count(item.quantity))
                .execute(&mut *self.conn)
                .await
                .map_err(error)?;
        }
        Ok(())
    }
}

fn bind_item(query: PgQuery<'_>, item: LineItem) -> PgQuery<'_> {
    query
        .bind(item.ticket_type_id.as_uuid())
        .bind(count(item.quantity))
}

#[async_trait]
impl InventoryTx for PostgresTx {
    async fn lock_stock(&mut self, items: &LineItems) -> StoreResult<Vec<u32>> {
        // One row per statement, so the locks are taken exactly in the items' order.
        let mut available = Vec::with_capacity(items.len());
        for item in items {
            let left = sqlx::query_scalar::<_, i64>(
                "SELECT capacity - held - sold FROM inventory WHERE ticket_type_id = $1 FOR UPDATE",
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
        self.all_or_nothing(
            items,
            "UPDATE inventory SET held = held + $2
             WHERE ticket_type_id = $1 AND held + sold + $2 <= capacity",
            "UPDATE inventory SET held = held - $2 WHERE ticket_type_id = $1",
            bind_item,
        )
        .await
    }

    async fn release_held(&mut self, items: &LineItems) -> StoreResult<()> {
        self.for_each_item(
            items,
            "UPDATE inventory SET held = held - $2 WHERE ticket_type_id = $1",
        )
        .await
    }

    async fn sell_held(&mut self, items: &LineItems) -> StoreResult<()> {
        self.for_each_item(
            items,
            "UPDATE inventory SET held = held - $2, sold = sold + $2 WHERE ticket_type_id = $1",
        )
        .await
    }

    async fn return_sold(&mut self, items: &LineItems) -> StoreResult<()> {
        // `CHECK (sold >= 0)` refuses returning more than was sold.
        self.for_each_item(
            items,
            "UPDATE inventory SET sold = sold - $2 WHERE ticket_type_id = $1",
        )
        .await
    }

    async fn try_sell(&mut self, items: &LineItems) -> StoreResult<bool> {
        self.all_or_nothing(
            items,
            "UPDATE inventory SET sold = sold + $2
             WHERE ticket_type_id = $1 AND held + sold + $2 <= capacity",
            "UPDATE inventory SET sold = sold - $2 WHERE ticket_type_id = $1",
            bind_item,
        )
        .await
    }

    async fn try_take_quota(&mut self, account: AccountId, holds: &Holds) -> StoreResult<bool> {
        for hold in holds {
            sqlx::query(
                "INSERT INTO quotas (account_id, ticket_type_id, taken) VALUES ($1, $2, 0)
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
            "UPDATE quotas SET taken = taken + $3
             WHERE account_id = $1 AND ticket_type_id = $2 AND taken + $3 <= $4",
            "UPDATE quotas SET taken = taken - $3 WHERE account_id = $1 AND ticket_type_id = $2",
            move |query, hold: Hold| {
                query
                    .bind(account)
                    .bind(hold.ticket_type_id.as_uuid())
                    .bind(count(hold.quantity))
                    .bind(count(hold.per_account_limit))
            },
        )
        .await
    }

    async fn return_quota(&mut self, account: AccountId, items: &LineItems) -> StoreResult<()> {
        for item in items {
            sqlx::query(
                "UPDATE quotas SET taken = taken - $3 WHERE account_id = $1 AND ticket_type_id = $2",
            )
            .bind(account.as_uuid())
            .bind(item.ticket_type_id.as_uuid())
            .bind(count(item.quantity))
            .execute(&mut *self.conn)
            .await
            .map_err(error)?;
        }
        Ok(())
    }

    async fn lock_quotas(&mut self, lines: &QuotaLines) -> StoreResult<Vec<u32>> {
        let (accounts, ticket_types): (Vec<Uuid>, Vec<Uuid>) = lines
            .iter()
            .map(|line| (line.account_id.as_uuid(), line.ticket_type_id.as_uuid()))
            .unzip();
        // Every row is inserted or updated in the lines' order, which locks it, quotas never
        // taken included; the update changes nothing and returns what was taken.
        let rows = sqlx::query_as::<_, (Uuid, Uuid, i64)>(
            "INSERT INTO quotas (account_id, ticket_type_id, taken)
             SELECT account_id, ticket_type_id, 0
             FROM unnest($1::uuid[], $2::uuid[]) WITH ORDINALITY
                  AS line (account_id, ticket_type_id, position)
             ORDER BY position
             ON CONFLICT (account_id, ticket_type_id) DO UPDATE SET taken = quotas.taken
             RETURNING account_id, ticket_type_id, taken",
        )
        .bind(accounts)
        .bind(ticket_types)
        .fetch_all(&mut *self.conn)
        .await
        .map_err(error)?;
        let taken: HashMap<(Uuid, Uuid), i64> = rows
            .into_iter()
            .map(|(account, ticket_type, taken)| ((account, ticket_type), taken))
            .collect();
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
        let mut accounts = Vec::with_capacity(lines.len());
        let mut ticket_types = Vec::with_capacity(lines.len());
        let mut quantities = Vec::with_capacity(lines.len());
        for line in lines {
            accounts.push(line.account_id.as_uuid());
            ticket_types.push(line.ticket_type_id.as_uuid());
            quantities.push(count(line.quantity));
        }
        sqlx::query(
            "INSERT INTO quotas (account_id, ticket_type_id, taken)
             SELECT * FROM unnest($1::uuid[], $2::uuid[], $3::bigint[])
             ON CONFLICT (account_id, ticket_type_id)
             DO UPDATE SET taken = quotas.taken + EXCLUDED.taken",
        )
        .bind(accounts)
        .bind(ticket_types)
        .bind(quantities)
        .execute(&mut *self.conn)
        .await
        .map_err(error)?;
        Ok(())
    }
}

#[async_trait]
impl ReservationsTx for PostgresTx {
    async fn insert_reservation(&mut self, reservation: &Reservation) -> StoreResult<()> {
        sqlx::query(
            "INSERT INTO reservations (id, purchase_request_id, account_id, sale_id, event_id, items,
                                       total_minor, currency, environment, status, attestor_id,
                                       expires_at, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)",
        )
        .bind(reservation.id.as_uuid())
        .bind(reservation.purchase_request_id.as_uuid())
        .bind(reservation.account_id.as_uuid())
        .bind(reservation.sale_id.as_uuid())
        .bind(reservation.event_id.as_uuid())
        .bind(Json(&reservation.items))
        .bind(reservation.total.amount_minor())
        .bind(reservation.total.currency().as_str())
        .bind(reservation.environment.as_str())
        .bind(reservation.status.as_str())
        .bind(reservation.attestor_id.map(|id| id.as_uuid()))
        .bind(at(reservation.expires_at))
        .bind(at(reservation.created_at))
        .bind(at(reservation.updated_at))
        .execute(&mut *self.conn)
        .await
        .map_err(unique("purchase_request_id"))?;
        Ok(())
    }

    async fn insert_reservations(&mut self, reservations: &[Reservation]) -> StoreResult<()> {
        let columns = ReservationColumns::new(reservations);
        sqlx::query(
            "INSERT INTO reservations (id, purchase_request_id, account_id, sale_id, event_id, items,
                                       total_minor, currency, environment, status, attestor_id,
                                       expires_at, created_at, updated_at)
             SELECT * FROM unnest($1::uuid[], $2::uuid[], $3::uuid[], $4::uuid[], $5::uuid[],
                                  $6::jsonb[], $7::bigint[], $8::text[], $9::text[], $10::text[],
                                  $11::uuid[], $12::timestamptz[], $13::timestamptz[],
                                  $14::timestamptz[])",
        )
        .bind(columns.ids)
        .bind(columns.purchase_requests)
        .bind(columns.accounts)
        .bind(columns.sales)
        .bind(columns.events)
        .bind(columns.items)
        .bind(columns.totals)
        .bind(columns.currencies)
        .bind(columns.environments)
        .bind(columns.statuses)
        .bind(columns.attestors)
        .bind(columns.expiries)
        .bind(columns.created)
        .bind(columns.updated)
        .execute(&mut *self.conn)
        .await
        .map_err(unique("purchase_request_id"))?;
        Ok(())
    }

    async fn lock_reservation(&mut self, id: ReservationId) -> StoreResult<Option<Reservation>> {
        let row = sqlx::query_as::<_, ReservationRow>(sqlx::AssertSqlSafe(format!(
            "{RESERVATION_COLUMNS} WHERE id = $1 FOR UPDATE"
        )))
        .bind(id.as_uuid())
        .fetch_optional(&mut *self.conn)
        .await
        .map_err(error)?;
        optional(row)
    }

    async fn update_reservation(&mut self, reservation: &Reservation) -> StoreResult<()> {
        let result = sqlx::query(
            "UPDATE reservations SET status = $2, attestor_id = $3, expires_at = $4, updated_at = $5
             WHERE id = $1",
        )
        .bind(reservation.id.as_uuid())
        .bind(reservation.status.as_str())
        .bind(reservation.attestor_id.map(|id| id.as_uuid()))
        .bind(at(reservation.expires_at))
        .bind(at(reservation.updated_at))
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

/// Reservations as one array per column, for inserting them with `unnest`.
struct ReservationColumns<'a> {
    ids: Vec<Uuid>,
    purchase_requests: Vec<Uuid>,
    accounts: Vec<Uuid>,
    sales: Vec<Uuid>,
    events: Vec<Uuid>,
    items: Vec<Json<&'a [ReservedItem]>>,
    totals: Vec<i64>,
    currencies: Vec<String>,
    environments: Vec<&'static str>,
    statuses: Vec<&'static str>,
    attestors: Vec<Option<Uuid>>,
    expiries: Vec<OffsetDateTime>,
    created: Vec<OffsetDateTime>,
    updated: Vec<OffsetDateTime>,
}

impl<'a> ReservationColumns<'a> {
    fn new(reservations: &'a [Reservation]) -> Self {
        let rows = reservations.len();
        let mut columns = Self {
            ids: Vec::with_capacity(rows),
            purchase_requests: Vec::with_capacity(rows),
            accounts: Vec::with_capacity(rows),
            sales: Vec::with_capacity(rows),
            events: Vec::with_capacity(rows),
            items: Vec::with_capacity(rows),
            totals: Vec::with_capacity(rows),
            currencies: Vec::with_capacity(rows),
            environments: Vec::with_capacity(rows),
            statuses: Vec::with_capacity(rows),
            attestors: Vec::with_capacity(rows),
            expiries: Vec::with_capacity(rows),
            created: Vec::with_capacity(rows),
            updated: Vec::with_capacity(rows),
        };
        for reservation in reservations {
            columns.ids.push(reservation.id.as_uuid());
            columns
                .purchase_requests
                .push(reservation.purchase_request_id.as_uuid());
            columns.accounts.push(reservation.account_id.as_uuid());
            columns.sales.push(reservation.sale_id.as_uuid());
            columns.events.push(reservation.event_id.as_uuid());
            columns.items.push(Json(reservation.items.as_slice()));
            columns.totals.push(reservation.total.amount_minor());
            columns
                .currencies
                .push(reservation.total.currency().as_str().to_owned());
            columns.environments.push(reservation.environment.as_str());
            columns.statuses.push(reservation.status.as_str());
            columns
                .attestors
                .push(reservation.attestor_id.map(|id| id.as_uuid()));
            columns.expiries.push(at(reservation.expires_at));
            columns.created.push(at(reservation.created_at));
            columns.updated.push(at(reservation.updated_at));
        }
        columns
    }
}
