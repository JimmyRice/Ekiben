//! Ticket types: what a sale sells, at what price and in what quantity.

use std::collections::BTreeMap;

use kippu_domain::catalog::{Event, TicketType};
use kippu_domain::{Money, SaleId, TicketTypeId, Timestamp, ValidationError};

use super::{TicketTypeAvailability, check_version, visible_sale, writable_sale};
use crate::app::AppState;
use crate::auth::Principal;
use crate::error::{ApiError, ApiResult};

/// A ticket type's settings.
#[derive(Debug, Clone)]
pub struct TicketTypeSettings {
    /// Display name, e.g. "Day 1".
    pub name: String,
    /// Price of one ticket.
    pub price: Money,
    /// How many tickets exist.
    pub capacity: u32,
    /// Most tickets of this type one account may hold.
    pub per_account_limit: u32,
    /// Tickets are valid from (inclusive).
    pub valid_from: Timestamp,
    /// Tickets are valid until (exclusive).
    pub valid_until: Timestamp,
    /// Extension claims written into every ticket, keyed by tag (128-255).
    pub ticket_extensions: BTreeMap<u8, String>,
    /// Until when buyers may refund tickets themselves; `None`: they may not.
    pub refundable_until: Option<Timestamp>,
}

/// Changes to a ticket type. `None` keeps the stored value.
#[derive(Debug, Clone, Default)]
pub struct TicketTypeChanges {
    /// Display name.
    pub name: Option<String>,
    /// Price of one ticket.
    pub price: Option<Money>,
    /// How many tickets exist; no lower than what is held and sold.
    pub capacity: Option<u32>,
    /// Most tickets of this type one account may hold.
    pub per_account_limit: Option<u32>,
    /// Tickets are valid from (inclusive).
    pub valid_from: Option<Timestamp>,
    /// Tickets are valid until (exclusive).
    pub valid_until: Option<Timestamp>,
    /// Extension claims, replaced as a whole.
    pub ticket_extensions: Option<BTreeMap<u8, String>>,
    /// Until when buyers may refund tickets themselves; `Some(None)` makes them
    /// non-refundable.
    pub refundable_until: Option<Option<Timestamp>>,
}

impl From<TicketTypeSettings> for TicketTypeChanges {
    /// Changes that replace every setting.
    fn from(settings: TicketTypeSettings) -> Self {
        Self {
            name: Some(settings.name),
            price: Some(settings.price),
            capacity: Some(settings.capacity),
            per_account_limit: Some(settings.per_account_limit),
            valid_from: Some(settings.valid_from),
            valid_until: Some(settings.valid_until),
            ticket_extensions: Some(settings.ticket_extensions),
            refundable_until: Some(settings.refundable_until),
        }
    }
}

/// A ticket type whose event the caller may see.
#[tracing::instrument(skip_all)]
pub async fn visible_ticket_type(
    state: &AppState,
    principal: Option<&Principal>,
    id: TicketTypeId,
) -> ApiResult<TicketType> {
    let ticket_type = state
        .store()
        .ticket_type(id)
        .await?
        .ok_or_else(|| ApiError::not_found("ticket type"))?;
    visible_sale(state, principal, ticket_type.sale_id).await?;
    Ok(ticket_type)
}

/// Checks that buyers' refunds of `ticket_type` end before `event` opens: after that a refund
/// could follow an admission, which Kippu cannot see.
fn check_refund_period(ticket_type: &TicketType, event: &Event) -> ApiResult<()> {
    match ticket_type.refundable_until {
        Some(until) if until > event.starts_at => Err(ValidationError::new(
            "refundable_until",
            "must not be after the event starts",
        )
        .into()),
        _ => Ok(()),
    }
}

/// A ticket type the caller may see, with its availability.
#[tracing::instrument(skip_all)]
pub async fn ticket_type_offer(
    state: &AppState,
    principal: Option<&Principal>,
    id: TicketTypeId,
) -> ApiResult<TicketTypeAvailability> {
    let ticket_type = visible_ticket_type(state, principal, id).await?;
    availability(state, ticket_type).await
}

/// A ticket type and how many of its tickets can still be reserved.
pub(super) async fn availability(
    state: &AppState,
    ticket_type: TicketType,
) -> ApiResult<TicketTypeAvailability> {
    let available = state
        .store()
        .inventory(ticket_type.id)
        .await?
        .map_or(0, |inventory| inventory.available());
    Ok(TicketTypeAvailability {
        ticket_type,
        available,
    })
}

/// A ticket type the caller may edit.
#[tracing::instrument(skip_all)]
pub async fn writable_ticket_type(
    state: &AppState,
    principal: &Principal,
    id: TicketTypeId,
) -> ApiResult<TicketType> {
    let ticket_type = state
        .store()
        .ticket_type(id)
        .await?
        .ok_or_else(|| ApiError::not_found("ticket type"))?;
    writable_sale(state, principal, ticket_type.sale_id).await?;
    Ok(ticket_type)
}

/// Adds a ticket type, and its inventory, to a sale.
#[tracing::instrument(skip_all)]
pub async fn create_ticket_type(
    state: &AppState,
    principal: &Principal,
    sale_id: SaleId,
    settings: TicketTypeSettings,
) -> ApiResult<TicketType> {
    let (sale, event) = writable_sale(state, principal, sale_id).await?;
    let ticket_type = TicketType {
        id: TicketTypeId::generate(),
        sale_id: sale.id,
        event_id: event.id,
        name: settings.name,
        price: settings.price,
        capacity: settings.capacity,
        per_account_limit: settings.per_account_limit,
        valid_from: settings.valid_from,
        valid_until: settings.valid_until,
        ticket_extensions: settings.ticket_extensions,
        refundable_until: settings.refundable_until,
        created_at: state.now(),
        version: 1,
    };
    ticket_type.validate()?;
    check_refund_period(&ticket_type, &event)?;
    state.store().insert_ticket_type(&ticket_type).await?;
    Ok(ticket_type)
}

/// Edits a ticket type. Capacity can drop no lower than what is held and sold.
/// `expected_version` is the version the caller read.
#[tracing::instrument(skip_all)]
pub async fn update_ticket_type(
    state: &AppState,
    principal: &Principal,
    id: TicketTypeId,
    expected_version: i64,
    changes: TicketTypeChanges,
) -> ApiResult<TicketType> {
    let current = state
        .store()
        .ticket_type(id)
        .await?
        .ok_or_else(|| ApiError::not_found("ticket type"))?;
    let (_, event) = writable_sale(state, principal, current.sale_id).await?;
    check_version(current.version, expected_version)?;
    let ticket_type = TicketType {
        name: changes.name.unwrap_or(current.name),
        price: changes.price.unwrap_or(current.price),
        capacity: changes.capacity.unwrap_or(current.capacity),
        per_account_limit: changes
            .per_account_limit
            .unwrap_or(current.per_account_limit),
        valid_from: changes.valid_from.unwrap_or(current.valid_from),
        valid_until: changes.valid_until.unwrap_or(current.valid_until),
        ticket_extensions: changes
            .ticket_extensions
            .unwrap_or(current.ticket_extensions),
        refundable_until: changes.refundable_until.unwrap_or(current.refundable_until),
        version: expected_version + 1,
        ..current
    };
    ticket_type.validate()?;
    check_refund_period(&ticket_type, &event)?;
    state
        .store()
        .update_ticket_type(&ticket_type, expected_version)
        .await?;
    Ok(ticket_type)
}
