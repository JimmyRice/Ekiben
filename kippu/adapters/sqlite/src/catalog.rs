use async_trait::async_trait;
use kippu_domain::admission::WaitingRoom;
use kippu_domain::catalog::{Event, Inventory, Sale, TicketType};
use kippu_domain::{AccountId, EventId, SaleId, TicketTypeId, Timestamp};
use kippu_store::{CatalogStore, EventFilter, PageRequest, StoreError, StoreResult};
use uuid::Uuid;

use crate::convert::{
    EventRow, InventoryRow, SaleRow, TicketTypeRow, all, event_status, extensions_json, json,
    micros, optional,
};
use crate::{SqliteStore, error, unique};

const EVENT_COLUMNS: &str = "SELECT id, organization_id, slug, title, description, venue, starts_at, \
     ends_at, status, created_at, updated_at, version FROM events";

const SALE_COLUMNS: &str = "SELECT id, event_id, name, opens_at, closes_at, admission, \
     reservation_ttl_seconds, max_tickets_per_request, accepted_attestors, environment, \
     created_at, version FROM sales";

const TICKET_TYPE_COLUMNS: &str = "SELECT id, sale_id, event_id, name, price_minor, currency, \
     capacity, per_account_limit, valid_from, valid_until, ticket_extensions, created_at, \
     version FROM ticket_types";

/// Fails with `Conflict("version")` unless exactly one row was updated.
fn expect_one_updated(result: &sqlx::sqlite::SqliteQueryResult) -> StoreResult<()> {
    if result.rows_affected() == 1 {
        Ok(())
    } else {
        Err(StoreError::Conflict("version"))
    }
}

#[async_trait]
impl CatalogStore for SqliteStore {
    async fn insert_event(&self, event: &Event) -> StoreResult<()> {
        sqlx::query(
            "INSERT INTO events (id, organization_id, slug, title, description, venue, starts_at,
                                 ends_at, status, created_at, updated_at, version)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        )
        .bind(event.id.as_uuid())
        .bind(event.organization_id.as_uuid())
        .bind(event.slug.as_str())
        .bind(&event.title)
        .bind(&event.description)
        .bind(&event.venue)
        .bind(micros(event.starts_at))
        .bind(micros(event.ends_at))
        .bind(event_status(event.status))
        .bind(micros(event.created_at))
        .bind(micros(event.updated_at))
        .bind(event.version)
        .execute(&self.writer)
        .await
        .map_err(unique("slug"))?;
        Ok(())
    }

    async fn update_event(&self, event: &Event, expected_version: i64) -> StoreResult<()> {
        let result = sqlx::query(
            "UPDATE events SET slug = ?2, title = ?3, description = ?4, venue = ?5, starts_at = ?6,
                               ends_at = ?7, status = ?8, updated_at = ?9, version = ?10
             WHERE id = ?1 AND version = ?11",
        )
        .bind(event.id.as_uuid())
        .bind(event.slug.as_str())
        .bind(&event.title)
        .bind(&event.description)
        .bind(&event.venue)
        .bind(micros(event.starts_at))
        .bind(micros(event.ends_at))
        .bind(event_status(event.status))
        .bind(micros(event.updated_at))
        .bind(event.version)
        .bind(expected_version)
        .execute(&self.writer)
        .await
        .map_err(unique("slug"))?;
        expect_one_updated(&result)
    }

    async fn event(&self, id: EventId) -> StoreResult<Option<Event>> {
        let row = sqlx::query_as::<_, EventRow>(sqlx::AssertSqlSafe(format!(
            "{EVENT_COLUMNS} WHERE id = ?1"
        )))
        .bind(id.as_uuid())
        .fetch_optional(&self.reader)
        .await
        .map_err(error)?;
        optional(row)
    }

    async fn list_events(&self, filter: EventFilter, page: PageRequest) -> StoreResult<Vec<Event>> {
        let rows = sqlx::query_as::<_, EventRow>(sqlx::AssertSqlSafe(format!(
            "{EVENT_COLUMNS}
             WHERE (?1 IS NULL OR organization_id = ?1)
               AND (?2 = 0 OR status IN ('published', 'cancelled'))
               AND (?3 IS NULL OR id > ?3)
             ORDER BY id LIMIT ?4"
        )))
        .bind(filter.organization.map(|id| id.as_uuid()))
        .bind(filter.public_only)
        .bind(page.after)
        .bind(page.limit)
        .fetch_all(&self.reader)
        .await
        .map_err(error)?;
        all(rows)
    }

    async fn insert_sale(&self, sale: &Sale) -> StoreResult<()> {
        sqlx::query(
            "INSERT INTO sales (id, event_id, name, opens_at, closes_at, admission,
                                reservation_ttl_seconds, max_tickets_per_request,
                                accepted_attestors, environment, created_at, version)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        )
        .bind(sale.id.as_uuid())
        .bind(sale.event_id.as_uuid())
        .bind(&sale.name)
        .bind(micros(sale.opens_at))
        .bind(micros(sale.closes_at))
        .bind(json(&sale.admission)?)
        .bind(sale.reservation_ttl_seconds)
        .bind(sale.max_tickets_per_request)
        .bind(json(&sale.accepted_attestors)?)
        .bind(sale.environment.as_str())
        .bind(micros(sale.created_at))
        .bind(sale.version)
        .execute(&self.writer)
        .await
        .map_err(error)?;
        Ok(())
    }

    async fn update_sale(&self, sale: &Sale, expected_version: i64) -> StoreResult<()> {
        let result = sqlx::query(
            "UPDATE sales SET name = ?2, opens_at = ?3, closes_at = ?4, admission = ?5,
                              reservation_ttl_seconds = ?6, max_tickets_per_request = ?7,
                              accepted_attestors = ?8, environment = ?9, version = ?10
             WHERE id = ?1 AND version = ?11",
        )
        .bind(sale.id.as_uuid())
        .bind(&sale.name)
        .bind(micros(sale.opens_at))
        .bind(micros(sale.closes_at))
        .bind(json(&sale.admission)?)
        .bind(sale.reservation_ttl_seconds)
        .bind(sale.max_tickets_per_request)
        .bind(json(&sale.accepted_attestors)?)
        .bind(sale.environment.as_str())
        .bind(sale.version)
        .bind(expected_version)
        .execute(&self.writer)
        .await
        .map_err(error)?;
        expect_one_updated(&result)
    }

    async fn sale(&self, id: SaleId) -> StoreResult<Option<Sale>> {
        let row = sqlx::query_as::<_, SaleRow>(sqlx::AssertSqlSafe(format!(
            "{SALE_COLUMNS} WHERE id = ?1"
        )))
        .bind(id.as_uuid())
        .fetch_optional(&self.reader)
        .await
        .map_err(error)?;
        optional(row)
    }

    async fn list_sales(&self, event: EventId) -> StoreResult<Vec<Sale>> {
        let rows = sqlx::query_as::<_, SaleRow>(sqlx::AssertSqlSafe(format!(
            "{SALE_COLUMNS} WHERE event_id = ?1 ORDER BY id"
        )))
        .bind(event.as_uuid())
        .fetch_all(&self.reader)
        .await
        .map_err(error)?;
        all(rows)
    }

    async fn insert_ticket_type(&self, ticket_type: &TicketType) -> StoreResult<()> {
        let mut tx = self
            .writer
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(error)?;
        sqlx::query(
            "INSERT INTO ticket_types (id, sale_id, event_id, name, price_minor, currency, capacity,
                                       per_account_limit, valid_from, valid_until,
                                       ticket_extensions, created_at, version)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        )
        .bind(ticket_type.id.as_uuid())
        .bind(ticket_type.sale_id.as_uuid())
        .bind(ticket_type.event_id.as_uuid())
        .bind(&ticket_type.name)
        .bind(ticket_type.price.amount_minor)
        .bind(ticket_type.price.currency.as_str())
        .bind(ticket_type.capacity)
        .bind(ticket_type.per_account_limit)
        .bind(micros(ticket_type.valid_from))
        .bind(micros(ticket_type.valid_until))
        .bind(extensions_json(&ticket_type.ticket_extensions)?)
        .bind(micros(ticket_type.created_at))
        .bind(ticket_type.version)
        .execute(&mut *tx)
        .await
        .map_err(error)?;
        sqlx::query(
            "INSERT INTO inventory (ticket_type_id, capacity, held, sold) VALUES (?1, ?2, 0, 0)",
        )
        .bind(ticket_type.id.as_uuid())
        .bind(ticket_type.capacity)
        .execute(&mut *tx)
        .await
        .map_err(error)?;
        tx.commit().await.map_err(error)
    }

    async fn update_ticket_type(
        &self,
        ticket_type: &TicketType,
        expected_version: i64,
    ) -> StoreResult<()> {
        let mut tx = self
            .writer
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(error)?;
        let result = sqlx::query(
            "UPDATE ticket_types SET name = ?2, price_minor = ?3, currency = ?4, capacity = ?5,
                                     per_account_limit = ?6, valid_from = ?7, valid_until = ?8,
                                     ticket_extensions = ?9, version = ?10
             WHERE id = ?1 AND version = ?11",
        )
        .bind(ticket_type.id.as_uuid())
        .bind(&ticket_type.name)
        .bind(ticket_type.price.amount_minor)
        .bind(ticket_type.price.currency.as_str())
        .bind(ticket_type.capacity)
        .bind(ticket_type.per_account_limit)
        .bind(micros(ticket_type.valid_from))
        .bind(micros(ticket_type.valid_until))
        .bind(extensions_json(&ticket_type.ticket_extensions)?)
        .bind(ticket_type.version)
        .bind(expected_version)
        .execute(&mut *tx)
        .await
        .map_err(error)?;
        expect_one_updated(&result)?;

        let resized = sqlx::query(
            "UPDATE inventory SET capacity = ?2 WHERE ticket_type_id = ?1 AND held + sold <= ?2",
        )
        .bind(ticket_type.id.as_uuid())
        .bind(ticket_type.capacity)
        .execute(&mut *tx)
        .await
        .map_err(error)?;
        if resized.rows_affected() != 1 {
            return Err(StoreError::Conflict("capacity"));
        }
        tx.commit().await.map_err(error)
    }

    async fn ticket_type(&self, id: TicketTypeId) -> StoreResult<Option<TicketType>> {
        let row = sqlx::query_as::<_, TicketTypeRow>(sqlx::AssertSqlSafe(format!(
            "{TICKET_TYPE_COLUMNS} WHERE id = ?1"
        )))
        .bind(id.as_uuid())
        .fetch_optional(&self.reader)
        .await
        .map_err(error)?;
        optional(row)
    }

    async fn list_ticket_types(&self, sale: SaleId) -> StoreResult<Vec<TicketType>> {
        let rows = sqlx::query_as::<_, TicketTypeRow>(sqlx::AssertSqlSafe(format!(
            "{TICKET_TYPE_COLUMNS} WHERE sale_id = ?1 ORDER BY id"
        )))
        .bind(sale.as_uuid())
        .fetch_all(&self.reader)
        .await
        .map_err(error)?;
        all(rows)
    }

    async fn inventory(&self, ticket_type: TicketTypeId) -> StoreResult<Option<Inventory>> {
        let row = sqlx::query_as::<_, InventoryRow>(
            "SELECT ticket_type_id, capacity, held, sold FROM inventory WHERE ticket_type_id = ?1",
        )
        .bind(ticket_type.as_uuid())
        .fetch_optional(&self.reader)
        .await
        .map_err(error)?;
        Ok(row.map(Inventory::from))
    }

    async fn add_favorite(
        &self,
        account: AccountId,
        event: EventId,
        now: Timestamp,
    ) -> StoreResult<()> {
        sqlx::query(
            "INSERT INTO favorites (account_id, event_id, created_at) VALUES (?1, ?2, ?3)
             ON CONFLICT DO NOTHING",
        )
        .bind(account.as_uuid())
        .bind(event.as_uuid())
        .bind(micros(now))
        .execute(&self.writer)
        .await
        .map_err(error)?;
        Ok(())
    }

    async fn remove_favorite(&self, account: AccountId, event: EventId) -> StoreResult<()> {
        sqlx::query("DELETE FROM favorites WHERE account_id = ?1 AND event_id = ?2")
            .bind(account.as_uuid())
            .bind(event.as_uuid())
            .execute(&self.writer)
            .await
            .map_err(error)?;
        Ok(())
    }

    async fn favorites(&self, account: AccountId) -> StoreResult<Vec<EventId>> {
        let ids = sqlx::query_scalar::<_, Uuid>(
            "SELECT event_id FROM favorites WHERE account_id = ?1 ORDER BY created_at DESC",
        )
        .bind(account.as_uuid())
        .fetch_all(&self.reader)
        .await
        .map_err(error)?;
        Ok(ids.into_iter().map(Into::into).collect())
    }

    async fn join_waiting_room(&self, sale: SaleId) -> StoreResult<u64> {
        let position = sqlx::query_scalar::<_, i64>(
            "INSERT INTO waiting_rooms (sale_id, last_position, admitted_through) VALUES (?1, 1, 0)
             ON CONFLICT (sale_id) DO UPDATE SET last_position = last_position + 1
             RETURNING last_position",
        )
        .bind(sale.as_uuid())
        .fetch_one(&self.writer)
        .await
        .map_err(error)?;
        u64::try_from(position).map_err(StoreError::backend)
    }

    async fn waiting_room(&self, sale: SaleId) -> StoreResult<WaitingRoom> {
        let row = sqlx::query_as::<_, (i64, i64)>(
            "SELECT last_position, admitted_through FROM waiting_rooms WHERE sale_id = ?1",
        )
        .bind(sale.as_uuid())
        .fetch_optional(&self.reader)
        .await
        .map_err(error)?;
        let Some((last_position, admitted_through)) = row else {
            return Ok(WaitingRoom::default());
        };
        Ok(WaitingRoom {
            last_position: u64::try_from(last_position).map_err(StoreError::backend)?,
            admitted_through: u64::try_from(admitted_through).map_err(StoreError::backend)?,
        })
    }

    async fn pending_waiting_rooms(&self) -> StoreResult<Vec<SaleId>> {
        let ids = sqlx::query_scalar::<_, Uuid>(
            "SELECT sale_id FROM waiting_rooms WHERE admitted_through < last_position ORDER BY sale_id",
        )
        .fetch_all(&self.reader)
        .await
        .map_err(error)?;
        Ok(ids.into_iter().map(Into::into).collect())
    }

    async fn advance_waiting_room(&self, sale: SaleId, from: u64, to: u64) -> StoreResult<bool> {
        let (from, to) = (
            i64::try_from(from).map_err(StoreError::backend)?,
            i64::try_from(to).map_err(StoreError::backend)?,
        );
        let result = sqlx::query(
            "UPDATE waiting_rooms SET admitted_through = ?3
             WHERE sale_id = ?1 AND admitted_through = ?2 AND ?3 <= last_position",
        )
        .bind(sale.as_uuid())
        .bind(from)
        .bind(to)
        .execute(&self.writer)
        .await
        .map_err(error)?;
        Ok(result.rows_affected() == 1)
    }
}
