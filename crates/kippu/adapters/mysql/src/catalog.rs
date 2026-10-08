use async_trait::async_trait;
use kippu_domain::admission::WaitingRoom;
use kippu_domain::catalog::{Event, EventSummary, Inventory, Sale, TicketType};
use kippu_domain::validation::CountryCode;
use kippu_domain::{AccountId, EventId, SaleId, TicketTypeId, Timestamp};
use kippu_store::{
    CatalogStore, EventFilter, EventOrder, Favorite, Keyset, PageRequest, StoreError, StoreResult,
};
use uuid::Uuid;

use crate::convert::{
    AddressValues, EventRow, EventSummaryRow, InventoryRow, SaleRow, TicketTypeRow, all,
    event_status, extensions_json, instant, json, micros, optional,
};
use crate::{MySqlStore, error, unique};

const EVENT_SUMMARY_COLUMNS: &str = "SELECT id, organization_id, slug, title, description, venue, \
     address_country, address_region, address_locality, address_postal_code, address_street, \
     latitude, longitude, starts_at, ends_at, status, created_at, updated_at, version FROM events";

const EVENT_COLUMNS: &str = "SELECT id, organization_id, slug, title, description, venue, \
     address_country, address_region, address_locality, address_postal_code, address_street, \
     latitude, longitude, starts_at, ends_at, status, created_at, updated_at, version, content \
     FROM events";

/// The column an event order sorts by, the comparison that finds the events after a
/// position, and the direction.
const fn event_order(order: EventOrder) -> (&'static str, &'static str, &'static str) {
    match order {
        EventOrder::StartsAt => ("starts_at", ">", "ASC"),
        EventOrder::StartsAtDesc => ("starts_at", "<", "DESC"),
        EventOrder::CreatedAt => ("created_at", ">", "ASC"),
        EventOrder::CreatedAtDesc => ("created_at", "<", "DESC"),
    }
}

const SALE_COLUMNS: &str = "SELECT id, event_id, name, opens_at, closes_at, admission, \
     reservation_ttl_seconds, max_tickets_per_request, accepted_attestors, environment, \
     created_at, version FROM sales";

const TICKET_TYPE_COLUMNS: &str = "SELECT id, sale_id, event_id, name, price_minor, currency, \
     capacity, per_account_limit, valid_from, valid_until, ticket_extensions, refundable_until, \
     created_at, version FROM ticket_types";

/// Fails with `Conflict("version")` unless exactly one row was updated.
fn expect_one_updated(result: &sqlx::mysql::MySqlQueryResult) -> StoreResult<()> {
    if result.rows_affected() == 1 {
        Ok(())
    } else {
        Err(StoreError::Conflict("version"))
    }
}

#[async_trait]
impl CatalogStore for MySqlStore {
    async fn insert_event(&self, event: &Event) -> StoreResult<()> {
        let address = AddressValues::of(event.address.as_ref());
        sqlx::query(
            "INSERT INTO events (id, organization_id, slug, title, description, venue, starts_at,
                                 ends_at, status, created_at, updated_at, version, content,
                                 address_country, address_region, address_locality,
                                 address_postal_code, address_street, latitude, longitude)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
        .bind(&event.content)
        .bind(address.country)
        .bind(address.region)
        .bind(address.locality)
        .bind(address.postal_code)
        .bind(address.street)
        .bind(address.latitude)
        .bind(address.longitude)
        .execute(&self.pool)
        .await
        .map_err(unique("slug"))?;
        Ok(())
    }

    async fn update_event(&self, event: &Event, expected_version: i64) -> StoreResult<()> {
        let address = AddressValues::of(event.address.as_ref());
        let result = sqlx::query(
            "UPDATE events SET slug = ?, title = ?, description = ?, venue = ?, starts_at = ?,
                               ends_at = ?, status = ?, updated_at = ?, version = ?,
                               content = ?, address_country = ?, address_region = ?,
                               address_locality = ?, address_postal_code = ?,
                               address_street = ?, latitude = ?, longitude = ?
             WHERE id = ? AND version = ?",
        )
        .bind(event.slug.as_str())
        .bind(&event.title)
        .bind(&event.description)
        .bind(&event.venue)
        .bind(micros(event.starts_at))
        .bind(micros(event.ends_at))
        .bind(event_status(event.status))
        .bind(micros(event.updated_at))
        .bind(event.version)
        .bind(&event.content)
        .bind(address.country)
        .bind(address.region)
        .bind(address.locality)
        .bind(address.postal_code)
        .bind(address.street)
        .bind(address.latitude)
        .bind(address.longitude)
        .bind(event.id.as_uuid())
        .bind(expected_version)
        .execute(&self.pool)
        .await
        .map_err(unique("slug"))?;
        expect_one_updated(&result)
    }

    async fn event(&self, id: EventId) -> StoreResult<Option<Event>> {
        let row = sqlx::query_as::<_, EventRow>(sqlx::AssertSqlSafe(format!(
            "{EVENT_COLUMNS} WHERE id = ?"
        )))
        .bind(id.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(error)?;
        optional(row)
    }

    async fn list_events(
        &self,
        filter: &EventFilter,
        order: EventOrder,
        page: PageRequest<Keyset>,
    ) -> StoreResult<Vec<EventSummary>> {
        let (column, after, direction) = event_order(order);
        let organization = filter.organization.map(|id| id.as_uuid());
        let status = filter.status.map(event_status);
        let country = filter.country.as_ref().map(CountryCode::as_str);
        let starts_from = filter.starts_from.map(micros);
        let starts_before = filter.starts_before.map(micros);
        let ends_from = filter.ends_from.map(micros);
        let ends_before = filter.ends_before.map(micros);
        let after_at = page.after.map(|after| micros(after.at));
        let rows = sqlx::query_as::<_, EventSummaryRow>(sqlx::AssertSqlSafe(format!(
            "{EVENT_SUMMARY_COLUMNS}
             WHERE (? IS NULL OR organization_id = ?)
               AND (? = 0 OR status IN ('published', 'cancelled'))
               AND (? IS NULL OR status = ?)
               AND (? IS NULL OR address_country = ?)
               AND (? IS NULL OR starts_at >= ?)
               AND (? IS NULL OR starts_at < ?)
               AND (? IS NULL OR ends_at >= ?)
               AND (? IS NULL OR ends_at < ?)
               AND (? IS NULL OR {column} {after} ? OR ({column} = ? AND id {after} ?))
             ORDER BY {column} {direction}, id {direction} LIMIT ?"
        )))
        .bind(organization)
        .bind(organization)
        .bind(filter.public_only)
        .bind(status)
        .bind(status)
        .bind(country)
        .bind(country)
        .bind(starts_from)
        .bind(starts_from)
        .bind(starts_before)
        .bind(starts_before)
        .bind(ends_from)
        .bind(ends_from)
        .bind(ends_before)
        .bind(ends_before)
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

    async fn insert_sale(&self, sale: &Sale) -> StoreResult<()> {
        sqlx::query(
            "INSERT INTO sales (id, event_id, name, opens_at, closes_at, admission,
                                reservation_ttl_seconds, max_tickets_per_request,
                                accepted_attestors, environment, created_at, version)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
        .execute(&self.pool)
        .await
        .map_err(error)?;
        Ok(())
    }

    async fn update_sale(&self, sale: &Sale, expected_version: i64) -> StoreResult<()> {
        let result = sqlx::query(
            "UPDATE sales SET name = ?, opens_at = ?, closes_at = ?, admission = ?,
                              reservation_ttl_seconds = ?, max_tickets_per_request = ?,
                              accepted_attestors = ?, environment = ?, version = ?
             WHERE id = ? AND version = ?",
        )
        .bind(&sale.name)
        .bind(micros(sale.opens_at))
        .bind(micros(sale.closes_at))
        .bind(json(&sale.admission)?)
        .bind(sale.reservation_ttl_seconds)
        .bind(sale.max_tickets_per_request)
        .bind(json(&sale.accepted_attestors)?)
        .bind(sale.environment.as_str())
        .bind(sale.version)
        .bind(sale.id.as_uuid())
        .bind(expected_version)
        .execute(&self.pool)
        .await
        .map_err(error)?;
        expect_one_updated(&result)
    }

    async fn sale(&self, id: SaleId) -> StoreResult<Option<Sale>> {
        let row = sqlx::query_as::<_, SaleRow>(sqlx::AssertSqlSafe(format!(
            "{SALE_COLUMNS} WHERE id = ?"
        )))
        .bind(id.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(error)?;
        optional(row)
    }

    async fn list_sales(&self, event: EventId) -> StoreResult<Vec<Sale>> {
        let rows = sqlx::query_as::<_, SaleRow>(sqlx::AssertSqlSafe(format!(
            "{SALE_COLUMNS} WHERE event_id = ? ORDER BY id"
        )))
        .bind(event.as_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(error)?;
        all(rows)
    }

    async fn insert_ticket_type(&self, ticket_type: &TicketType) -> StoreResult<()> {
        let mut tx = self.pool.begin().await.map_err(error)?;
        sqlx::query(
            "INSERT INTO ticket_types (id, sale_id, event_id, name, price_minor, currency, capacity,
                                       per_account_limit, valid_from, valid_until,
                                       ticket_extensions, refundable_until, created_at,
                                       version)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(ticket_type.id.as_uuid())
        .bind(ticket_type.sale_id.as_uuid())
        .bind(ticket_type.event_id.as_uuid())
        .bind(&ticket_type.name)
        .bind(ticket_type.price.amount_minor())
        .bind(ticket_type.price.currency().as_str())
        .bind(ticket_type.capacity)
        .bind(ticket_type.per_account_limit)
        .bind(micros(ticket_type.valid_from))
        .bind(micros(ticket_type.valid_until))
        .bind(extensions_json(&ticket_type.ticket_extensions)?)
        .bind(ticket_type.refundable_until.map(micros))
        .bind(micros(ticket_type.created_at))
        .bind(ticket_type.version)
        .execute(&mut *tx)
        .await
        .map_err(error)?;
        sqlx::query(
            "INSERT INTO inventory (ticket_type_id, capacity, held, sold) VALUES (?, ?, 0, 0)",
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
        let mut tx = self.pool.begin().await.map_err(error)?;
        let result = sqlx::query(
            "UPDATE ticket_types SET name = ?, price_minor = ?, currency = ?, capacity = ?,
                                     per_account_limit = ?, valid_from = ?, valid_until = ?,
                                     ticket_extensions = ?, refundable_until = ?, version = ?
             WHERE id = ? AND version = ?",
        )
        .bind(&ticket_type.name)
        .bind(ticket_type.price.amount_minor())
        .bind(ticket_type.price.currency().as_str())
        .bind(ticket_type.capacity)
        .bind(ticket_type.per_account_limit)
        .bind(micros(ticket_type.valid_from))
        .bind(micros(ticket_type.valid_until))
        .bind(extensions_json(&ticket_type.ticket_extensions)?)
        .bind(ticket_type.refundable_until.map(micros))
        .bind(ticket_type.version)
        .bind(ticket_type.id.as_uuid())
        .bind(expected_version)
        .execute(&mut *tx)
        .await
        .map_err(error)?;
        expect_one_updated(&result)?;

        let resized = sqlx::query(
            "UPDATE inventory SET capacity = ? WHERE ticket_type_id = ? AND held + sold <= ?",
        )
        .bind(ticket_type.capacity)
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
            "{TICKET_TYPE_COLUMNS} WHERE id = ?"
        )))
        .bind(id.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(error)?;
        optional(row)
    }

    async fn list_ticket_types(&self, sale: SaleId) -> StoreResult<Vec<TicketType>> {
        let rows = sqlx::query_as::<_, TicketTypeRow>(sqlx::AssertSqlSafe(format!(
            "{TICKET_TYPE_COLUMNS} WHERE sale_id = ? ORDER BY id"
        )))
        .bind(sale.as_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(error)?;
        all(rows)
    }

    async fn inventory(&self, ticket_type: TicketTypeId) -> StoreResult<Option<Inventory>> {
        let row = sqlx::query_as::<_, InventoryRow>(
            "SELECT ticket_type_id, capacity, held, sold FROM inventory WHERE ticket_type_id = ?",
        )
        .bind(ticket_type.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(error)?;
        optional(row)
    }

    async fn add_favorite(
        &self,
        account: AccountId,
        event: EventId,
        now: Timestamp,
    ) -> StoreResult<()> {
        sqlx::query(
            "INSERT INTO favorites (account_id, event_id, created_at) VALUES (?, ?, ?)
             ON DUPLICATE KEY UPDATE created_at = created_at",
        )
        .bind(account.as_uuid())
        .bind(event.as_uuid())
        .bind(micros(now))
        .execute(&self.pool)
        .await
        .map_err(error)?;
        Ok(())
    }

    async fn remove_favorite(&self, account: AccountId, event: EventId) -> StoreResult<()> {
        sqlx::query("DELETE FROM favorites WHERE account_id = ? AND event_id = ?")
            .bind(account.as_uuid())
            .bind(event.as_uuid())
            .execute(&self.pool)
            .await
            .map_err(error)?;
        Ok(())
    }

    async fn favorites(
        &self,
        account: AccountId,
        page: PageRequest<Keyset>,
    ) -> StoreResult<Vec<Favorite>> {
        let after_at = page.after.map(|after| micros(after.at));
        let rows = sqlx::query_as::<_, (Uuid, i64)>(
            "SELECT event_id, created_at FROM favorites
             WHERE account_id = ?
               AND (? IS NULL OR created_at < ? OR (created_at = ? AND event_id > ?))
             ORDER BY created_at DESC, event_id LIMIT ?",
        )
        .bind(account.as_uuid())
        .bind(after_at)
        .bind(after_at)
        .bind(after_at)
        .bind(page.after.map(|after| after.id))
        .bind(page.limit)
        .fetch_all(&self.pool)
        .await
        .map_err(error)?;
        Ok(rows
            .into_iter()
            .map(|(event_id, created_at)| Favorite {
                event_id: event_id.into(),
                created_at: instant(created_at),
            })
            .collect())
    }

    async fn join_waiting_room(&self, sale: SaleId) -> StoreResult<u64> {
        // LAST_INSERT_ID(expr) hands the new position back on this connection, atomically
        // with the increment.
        let result = sqlx::query(
            "INSERT INTO waiting_rooms (sale_id, last_position, admitted_through)
             VALUES (?, LAST_INSERT_ID(1), 0)
             ON DUPLICATE KEY UPDATE last_position = LAST_INSERT_ID(last_position + 1)",
        )
        .bind(sale.as_uuid())
        .execute(&self.pool)
        .await
        .map_err(error)?;
        Ok(result.last_insert_id())
    }

    async fn waiting_room(&self, sale: SaleId) -> StoreResult<WaitingRoom> {
        let row = sqlx::query_as::<_, (i64, i64)>(
            "SELECT last_position, admitted_through FROM waiting_rooms WHERE sale_id = ?",
        )
        .bind(sale.as_uuid())
        .fetch_optional(&self.pool)
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
        .fetch_all(&self.pool)
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
            "UPDATE waiting_rooms SET admitted_through = ?
             WHERE sale_id = ? AND admitted_through = ? AND ? <= last_position",
        )
        .bind(to)
        .bind(sale.as_uuid())
        .bind(from)
        .bind(to)
        .execute(&self.pool)
        .await
        .map_err(error)?;
        Ok(result.rows_affected() == 1)
    }
}
