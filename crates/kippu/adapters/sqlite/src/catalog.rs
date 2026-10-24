use async_trait::async_trait;
use kippu_domain::admission::{AdmissionPolicy, WaitingRoom};
use kippu_domain::catalog::{Event, EventSummary, Inventory, Sale, TicketType};
use kippu_domain::validation::{CountryCode, Slug};
use kippu_domain::{AccountId, EventId, SaleId, TicketTypeId, Timestamp};
use kippu_store::{
    CatalogStore, EventFilter, EventOrder, Favorite, Keyset, PageRequest, StoreError, StoreResult,
};
use uuid::Uuid;

use crate::convert::{
    AddressValues, EventRow, EventSummaryRow, InventoryRow, SaleRow, TicketTypeRow, all,
    event_status, extensions_json, instant, json, micros, optional,
};
use crate::{SqliteStore, error, unique, waiting_rooms};

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

const SALE_COLUMNS: &str = "SELECT id, event_id, name, audience, opens_at, closes_at, admission, \
     reservation_ttl_seconds, max_tickets_per_request, accepted_attestors, environment, \
     created_at, version FROM sales";

const TICKET_TYPE_COLUMNS: &str = "SELECT id, sale_id, event_id, name, price_minor, currency, \
     capacity, per_account_limit, valid_from, valid_until, ticket_extensions, refundable_until, \
     created_at, version FROM ticket_types";

/// Fails with `Conflict("version")` unless exactly one row was updated.
pub(crate) fn expect_one_updated(result: &sqlx::sqlite::SqliteQueryResult) -> StoreResult<()> {
    if result.rows_affected() == 1 {
        Ok(())
    } else {
        Err(StoreError::Conflict("version"))
    }
}

#[async_trait]
impl CatalogStore for SqliteStore {
    async fn insert_event(&self, event: &Event) -> StoreResult<()> {
        let address = AddressValues::of(event.address.as_ref());
        sqlx::query(
            "INSERT INTO events (id, organization_id, slug, title, description, venue, starts_at,
                                 ends_at, status, created_at, updated_at, version, content,
                                 address_country, address_region, address_locality,
                                 address_postal_code, address_street, latitude, longitude)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17,
                     ?18, ?19, ?20)",
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
        .execute(&self.writer)
        .await
        .map_err(unique("slug"))?;
        Ok(())
    }

    async fn update_event(&self, event: &Event, expected_version: i64) -> StoreResult<()> {
        let address = AddressValues::of(event.address.as_ref());
        let result = sqlx::query(
            "UPDATE events SET slug = ?2, title = ?3, description = ?4, venue = ?5, starts_at = ?6,
                               ends_at = ?7, status = ?8, updated_at = ?9, version = ?10,
                               content = ?12, address_country = ?13, address_region = ?14,
                               address_locality = ?15, address_postal_code = ?16,
                               address_street = ?17, latitude = ?18, longitude = ?19
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
        .bind(&event.content)
        .bind(address.country)
        .bind(address.region)
        .bind(address.locality)
        .bind(address.postal_code)
        .bind(address.street)
        .bind(address.latitude)
        .bind(address.longitude)
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

    async fn event_by_slug(&self, slug: &Slug) -> StoreResult<Option<Event>> {
        let row = sqlx::query_as::<_, EventRow>(sqlx::AssertSqlSafe(format!(
            "{EVENT_COLUMNS} WHERE slug = ?1"
        )))
        .bind(slug.as_str())
        .fetch_optional(&self.reader)
        .await
        .map_err(error)?;
        optional(row)
    }

    async fn event_summary(&self, id: EventId) -> StoreResult<Option<EventSummary>> {
        let row = sqlx::query_as::<_, EventSummaryRow>(sqlx::AssertSqlSafe(format!(
            "{EVENT_SUMMARY_COLUMNS} WHERE id = ?1"
        )))
        .bind(id.as_uuid())
        .fetch_optional(&self.reader)
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
        let rows = sqlx::query_as::<_, EventSummaryRow>(sqlx::AssertSqlSafe(format!(
            "{EVENT_SUMMARY_COLUMNS}
             WHERE (?1 IS NULL OR organization_id = ?1)
               AND (?2 = 0 OR status IN ('published', 'cancelled'))
               AND (?3 IS NULL OR status = ?3)
               AND (?4 IS NULL OR address_country = ?4)
               AND (?5 IS NULL OR starts_at >= ?5)
               AND (?6 IS NULL OR starts_at < ?6)
               AND (?7 IS NULL OR ends_at >= ?7)
               AND (?8 IS NULL OR ends_at < ?8)
               AND (?9 IS NULL OR {column} {after} ?9 OR ({column} = ?9 AND id {after} ?10))
             ORDER BY {column} {direction}, id {direction} LIMIT ?11"
        )))
        .bind(filter.organization.map(|id| id.as_uuid()))
        .bind(filter.public_only)
        .bind(filter.status.map(event_status))
        .bind(filter.country.as_ref().map(CountryCode::as_str))
        .bind(filter.starts_from.map(micros))
        .bind(filter.starts_before.map(micros))
        .bind(filter.ends_from.map(micros))
        .bind(filter.ends_before.map(micros))
        .bind(page.after.map(|after| micros(after.at)))
        .bind(page.after.map(|after| after.id))
        .bind(page.limit)
        .fetch_all(&self.reader)
        .await
        .map_err(error)?;
        all(rows)
    }

    async fn delete_draft_event(&self, id: EventId) -> StoreResult<bool> {
        let mut tx = self
            .writer
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(error)?;
        // Denials name the event without cascading; images and favourites cascade.
        sqlx::query("DELETE FROM denials WHERE event_id = ?1")
            .bind(id.as_uuid())
            .execute(&mut *tx)
            .await
            .map_err(error)?;
        let deleted = sqlx::query(
            "DELETE FROM events WHERE id = ?1 AND status = 'draft'
               AND NOT EXISTS (SELECT 1 FROM sales WHERE event_id = ?1)",
        )
        .bind(id.as_uuid())
        .execute(&mut *tx)
        .await
        .map_err(error);
        match deleted {
            Ok(result) if result.rows_affected() == 1 => {
                tx.commit().await.map_err(error)?;
                Ok(true)
            }
            // A sale committed meanwhile still references the event.
            Ok(_) | Err(StoreError::Conflict("reference")) => Ok(false),
            Err(other) => Err(other),
        }
    }

    async fn insert_sale(&self, sale: &Sale) -> StoreResult<()> {
        // A sale's waiting room, if it has one, comes with it in the same transaction; a sale
        // that takes one later gets it on the first join.
        let mut tx = self
            .writer
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(error)?;
        sqlx::query(
            "INSERT INTO sales (id, event_id, name, opens_at, closes_at, admission,
                                reservation_ttl_seconds, max_tickets_per_request,
                                accepted_attestors, environment, created_at, version, audience)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
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
        .bind(sale.audience.as_str())
        .execute(&mut *tx)
        .await
        .map_err(error)?;
        if matches!(sale.admission, AdmissionPolicy::WaitingRoom { .. }) {
            waiting_rooms::create(&mut tx, sale.id).await?;
        }
        tx.commit().await.map_err(error)
    }

    async fn update_sale(&self, sale: &Sale, expected_version: i64) -> StoreResult<()> {
        let result = sqlx::query(
            "UPDATE sales SET name = ?2, opens_at = ?3, closes_at = ?4, admission = ?5,
                              reservation_ttl_seconds = ?6, max_tickets_per_request = ?7,
                              accepted_attestors = ?8, environment = ?9, version = ?10,
                              audience = ?12
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
        .bind(sale.audience.as_str())
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
                                       ticket_extensions, created_at, version, refundable_until)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
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
        .bind(micros(ticket_type.created_at))
        .bind(ticket_type.version)
        .bind(ticket_type.refundable_until.map(micros))
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
                                     ticket_extensions = ?9, version = ?10,
                                     refundable_until = ?12
             WHERE id = ?1 AND version = ?11",
        )
        .bind(ticket_type.id.as_uuid())
        .bind(&ticket_type.name)
        .bind(ticket_type.price.amount_minor())
        .bind(ticket_type.price.currency().as_str())
        .bind(ticket_type.capacity)
        .bind(ticket_type.per_account_limit)
        .bind(micros(ticket_type.valid_from))
        .bind(micros(ticket_type.valid_until))
        .bind(extensions_json(&ticket_type.ticket_extensions)?)
        .bind(ticket_type.version)
        .bind(expected_version)
        .bind(ticket_type.refundable_until.map(micros))
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

    async fn sale_inventory(&self, sale: SaleId) -> StoreResult<Vec<Inventory>> {
        let rows = sqlx::query_as::<_, InventoryRow>(
            "SELECT inventory.ticket_type_id, inventory.capacity, inventory.held, inventory.sold
             FROM inventory JOIN ticket_types ON ticket_types.id = inventory.ticket_type_id
             WHERE ticket_types.sale_id = ?1 ORDER BY inventory.ticket_type_id",
        )
        .bind(sale.as_uuid())
        .fetch_all(&self.reader)
        .await
        .map_err(error)?;
        Ok(rows.into_iter().map(Inventory::from).collect())
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

    async fn is_favorite(&self, account: AccountId, event: EventId) -> StoreResult<bool> {
        let found = sqlx::query_scalar::<_, i64>(
            "SELECT EXISTS (SELECT 1 FROM favorites WHERE account_id = ?1 AND event_id = ?2)",
        )
        .bind(account.as_uuid())
        .bind(event.as_uuid())
        .fetch_one(&self.reader)
        .await
        .map_err(error)?;
        Ok(found != 0)
    }

    async fn favorites(
        &self,
        account: AccountId,
        page: PageRequest<Keyset>,
    ) -> StoreResult<Vec<Favorite>> {
        let rows = sqlx::query_as::<_, (Uuid, i64)>(
            "SELECT event_id, created_at FROM favorites
             WHERE account_id = ?1
               AND (?2 IS NULL OR created_at < ?2 OR (created_at = ?2 AND event_id > ?3))
             ORDER BY created_at DESC, event_id LIMIT ?4",
        )
        .bind(account.as_uuid())
        .bind(page.after.map(|after| micros(after.at)))
        .bind(page.after.map(|after| after.id))
        .bind(page.limit)
        .fetch_all(&self.reader)
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

    async fn join_waiting_room(&self, sale: SaleId, account: AccountId) -> StoreResult<u64> {
        waiting_rooms::join(&self.reader, &self.writer, sale, account).await
    }

    async fn waiting_room(&self, sale: SaleId) -> StoreResult<WaitingRoom> {
        waiting_rooms::room(&self.reader, sale).await
    }

    async fn admitted_through(&self, sale: SaleId) -> StoreResult<u64> {
        waiting_rooms::admitted_through(&self.reader, sale).await
    }

    async fn pending_waiting_rooms(&self, now: Timestamp) -> StoreResult<Vec<SaleId>> {
        waiting_rooms::pending(&self.reader, now).await
    }

    async fn advance_waiting_room(
        &self,
        sale: SaleId,
        from: u64,
        to: u64,
    ) -> StoreResult<Option<u64>> {
        waiting_rooms::advance(&self.writer, sale, from, to).await
    }
}
