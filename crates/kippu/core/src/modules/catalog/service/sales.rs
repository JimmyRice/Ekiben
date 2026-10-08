//! Sales: the windows in which an event's tickets are sold.

use kippu_domain::admission::AdmissionPolicy;
use kippu_domain::catalog::{EventSummary, Sale, TicketType};
use kippu_domain::payment::Environment;
use kippu_domain::{AttestorId, EventId, SaleId, Timestamp};

use super::{availability, check_version, visible_event, writable_event};
use crate::app::AppState;
use crate::auth::Principal;
use crate::error::{ApiError, ApiResult};

/// A sale's settings.
#[derive(Debug, Clone)]
pub struct SaleSettings {
    /// Display name, e.g. "Early bird".
    pub name: String,
    /// When purchase requests start being accepted.
    pub opens_at: Timestamp,
    /// When purchase requests stop being accepted.
    pub closes_at: Timestamp,
    /// How buyers are let in when demand is high.
    pub admission: AdmissionPolicy,
    /// How long an unpaid reservation holds tickets.
    pub reservation_ttl_seconds: u32,
    /// Most tickets per purchase request.
    pub max_tickets_per_request: u32,
    /// Attestors whose payments settle this sale's reservations.
    pub accepted_attestors: Vec<AttestorId>,
    /// Whether the sale takes real money.
    pub environment: Environment,
}

/// Changes to a sale. `None` keeps the stored value.
#[derive(Debug, Clone, Default)]
pub struct SaleChanges {
    /// Display name.
    pub name: Option<String>,
    /// When purchase requests start being accepted.
    pub opens_at: Option<Timestamp>,
    /// When purchase requests stop being accepted.
    pub closes_at: Option<Timestamp>,
    /// The admission policy, replaced as a whole.
    pub admission: Option<AdmissionPolicy>,
    /// How long an unpaid reservation holds tickets.
    pub reservation_ttl_seconds: Option<u32>,
    /// Most tickets per purchase request.
    pub max_tickets_per_request: Option<u32>,
    /// Accepted attestors, replaced as a whole.
    pub accepted_attestors: Option<Vec<AttestorId>>,
    /// Whether the sale takes real money.
    pub environment: Option<Environment>,
}

impl From<SaleSettings> for SaleChanges {
    /// Changes that replace every setting.
    fn from(settings: SaleSettings) -> Self {
        Self {
            name: Some(settings.name),
            opens_at: Some(settings.opens_at),
            closes_at: Some(settings.closes_at),
            admission: Some(settings.admission),
            reservation_ttl_seconds: Some(settings.reservation_ttl_seconds),
            max_tickets_per_request: Some(settings.max_tickets_per_request),
            accepted_attestors: Some(settings.accepted_attestors),
            environment: Some(settings.environment),
        }
    }
}

/// A sale with its ticket types and what is left of each.
#[derive(Debug, Clone)]
pub struct SaleOffer {
    /// The sale.
    pub sale: Sale,
    /// What it sells.
    pub ticket_types: Vec<TicketTypeAvailability>,
}

/// A ticket type and how many of its tickets can still be reserved.
#[derive(Debug, Clone)]
pub struct TicketTypeAvailability {
    /// The ticket type.
    pub ticket_type: TicketType,
    /// Tickets that can be reserved right now.
    pub available: u32,
}

/// A sale whose event the caller may see.
#[tracing::instrument(skip_all)]
pub async fn visible_sale(
    state: &AppState,
    principal: Option<&Principal>,
    id: SaleId,
) -> ApiResult<(Sale, EventSummary)> {
    let sale = state
        .store()
        .sale(id)
        .await?
        .ok_or_else(|| ApiError::not_found("sale"))?;
    let event = visible_event(state, principal, sale.event_id).await?;
    Ok((sale, event))
}

/// A sale the caller may edit.
#[tracing::instrument(skip_all)]
pub async fn writable_sale(
    state: &AppState,
    principal: &Principal,
    id: SaleId,
) -> ApiResult<(Sale, EventSummary)> {
    let sale = state
        .store()
        .sale(id)
        .await?
        .ok_or_else(|| ApiError::not_found("sale"))?;
    let event = writable_event(state, principal, sale.event_id).await?;
    Ok((sale, event))
}

/// A sale with its ticket types and their availability.
async fn offer(state: &AppState, sale: Sale) -> ApiResult<SaleOffer> {
    let mut ticket_types = Vec::new();
    for ticket_type in state.store().list_ticket_types(sale.id).await? {
        ticket_types.push(availability(state, ticket_type).await?);
    }
    Ok(SaleOffer { sale, ticket_types })
}

/// The sales of an event the caller may see, with availability.
#[tracing::instrument(skip_all)]
pub async fn event_sales(
    state: &AppState,
    principal: Option<&Principal>,
    event_id: EventId,
) -> ApiResult<Vec<SaleOffer>> {
    let event = visible_event(state, principal, event_id).await?;
    let mut sales = Vec::new();
    for sale in state.store().list_sales(event.id).await? {
        sales.push(offer(state, sale).await?);
    }
    Ok(sales)
}

/// A sale the caller may see, with its ticket types and availability.
#[tracing::instrument(skip_all)]
pub async fn sale_offer(
    state: &AppState,
    principal: Option<&Principal>,
    id: SaleId,
) -> ApiResult<SaleOffer> {
    let (sale, _) = visible_sale(state, principal, id).await?;
    offer(state, sale).await
}

/// Creates a sale for an event.
#[tracing::instrument(skip_all)]
pub async fn create_sale(
    state: &AppState,
    principal: &Principal,
    event_id: EventId,
    settings: SaleSettings,
) -> ApiResult<Sale> {
    let event = writable_event(state, principal, event_id).await?;
    let sale = Sale {
        id: SaleId::generate(),
        event_id: event.id,
        name: settings.name,
        opens_at: settings.opens_at,
        closes_at: settings.closes_at,
        admission: settings.admission,
        reservation_ttl_seconds: settings.reservation_ttl_seconds,
        max_tickets_per_request: settings.max_tickets_per_request,
        accepted_attestors: settings.accepted_attestors,
        environment: settings.environment,
        created_at: state.now(),
        version: 1,
    };
    sale.validate()?;
    state.store().insert_sale(&sale).await?;
    Ok(sale)
}

/// Edits a sale. `expected_version` is the version the caller read.
#[tracing::instrument(skip_all)]
pub async fn update_sale(
    state: &AppState,
    principal: &Principal,
    id: SaleId,
    expected_version: i64,
    changes: SaleChanges,
) -> ApiResult<Sale> {
    let (current, _) = writable_sale(state, principal, id).await?;
    check_version(current.version, expected_version)?;
    let sale = Sale {
        name: changes.name.unwrap_or(current.name),
        opens_at: changes.opens_at.unwrap_or(current.opens_at),
        closes_at: changes.closes_at.unwrap_or(current.closes_at),
        admission: changes.admission.unwrap_or(current.admission),
        reservation_ttl_seconds: changes
            .reservation_ttl_seconds
            .unwrap_or(current.reservation_ttl_seconds),
        max_tickets_per_request: changes
            .max_tickets_per_request
            .unwrap_or(current.max_tickets_per_request),
        accepted_attestors: changes
            .accepted_attestors
            .unwrap_or(current.accepted_attestors),
        environment: changes.environment.unwrap_or(current.environment),
        version: expected_version + 1,
        ..current
    };
    sale.validate()?;
    state.store().update_sale(&sale, expected_version).await?;
    Ok(sale)
}
