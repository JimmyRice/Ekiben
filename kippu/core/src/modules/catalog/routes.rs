use std::collections::BTreeMap;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use kippu_domain::catalog::{Event, EventStatus, Sale, TicketType};
use kippu_domain::validation::Slug;
use kippu_domain::{EventId, OrganizationId, SaleId, TicketTypeId, ValidationError};
use kippu_store::EventFilter;

use super::dto::{
    CreateEventRequest, SaleDetail, SaleRequest, TicketTypeRequest, UpdateEventRequest,
};
use super::permissions::{EVENTS_WRITE, FAVORITES_MANAGE};
use super::service::{sale_detail, visible_event, visible_sale, writable_event, writable_sale};
use crate::app::AppState;
use crate::auth::{Principal, Scope};
use crate::error::{ApiError, ApiResult, Problem};
use crate::http::PageQuery;

const TAG: &str = "catalog";

fn version_of(version: Option<i64>) -> ApiResult<i64> {
    version.ok_or_else(|| ValidationError::new("version", "is required when updating").into())
}

/// Published events, for everyone.
#[utoipa::path(
    get, path = "/v1/events", tag = TAG,
    params(PageQuery),
    responses((status = 200, body = Vec<Event>))
)]
pub(crate) async fn list_events(
    State(state): State<AppState>,
    Query(query): Query<PageQuery>,
) -> ApiResult<Json<Vec<Event>>> {
    let filter = EventFilter {
        organization: None,
        public_only: true,
    };
    Ok(Json(state.store().list_events(filter, query.page()).await?))
}

/// All events of an organization, drafts included.
#[utoipa::path(
    get, path = "/v1/organizations/{organization_id}/events", tag = TAG,
    security(("bearer" = [])),
    params(("organization_id" = OrganizationId, Path), PageQuery),
    responses((status = 200, body = Vec<Event>), (status = 403, body = Problem))
)]
pub(crate) async fn list_organization_events(
    State(state): State<AppState>,
    principal: Principal,
    Path(organization_id): Path<OrganizationId>,
    Query(query): Query<PageQuery>,
) -> ApiResult<Json<Vec<Event>>> {
    state.authorize(
        &principal,
        EVENTS_WRITE,
        Scope::Organization(organization_id),
    )?;
    let filter = EventFilter {
        organization: Some(organization_id),
        public_only: false,
    };
    Ok(Json(state.store().list_events(filter, query.page()).await?))
}

/// Create a draft event.
#[utoipa::path(
    post, path = "/v1/organizations/{organization_id}/events", tag = TAG,
    security(("bearer" = [])),
    params(("organization_id" = OrganizationId, Path)),
    request_body = CreateEventRequest,
    responses((status = 201, body = Event), (status = 403, body = Problem), (status = 409, body = Problem))
)]
pub(crate) async fn create_event(
    State(state): State<AppState>,
    principal: Principal,
    Path(organization_id): Path<OrganizationId>,
    Json(request): Json<CreateEventRequest>,
) -> ApiResult<(StatusCode, Json<Event>)> {
    state.authorize(
        &principal,
        EVENTS_WRITE,
        Scope::Organization(organization_id),
    )?;
    state
        .store()
        .organization(organization_id)
        .await?
        .ok_or_else(|| ApiError::not_found("organization"))?;
    let now = state.now();
    let event = Event {
        id: EventId::generate(),
        organization_id,
        slug: Slug::new(request.slug)?,
        title: request.title,
        description: request.description,
        venue: request.venue,
        starts_at: request.starts_at,
        ends_at: request.ends_at,
        status: EventStatus::Draft,
        created_at: now,
        updated_at: now,
        version: 1,
    };
    event.validate()?;
    state.store().insert_event(&event).await?;
    Ok((StatusCode::CREATED, Json(event)))
}

/// An event.
#[utoipa::path(
    get, path = "/v1/events/{event_id}", tag = TAG,
    params(("event_id" = EventId, Path)),
    responses((status = 200, body = Event), (status = 404, body = Problem))
)]
pub(crate) async fn get_event(
    State(state): State<AppState>,
    principal: Option<Principal>,
    Path(event_id): Path<EventId>,
) -> ApiResult<Json<Event>> {
    Ok(Json(
        visible_event(&state, principal.as_ref(), event_id).await?,
    ))
}

/// Edit an event, including publishing or cancelling it.
#[utoipa::path(
    put, path = "/v1/events/{event_id}", tag = TAG,
    security(("bearer" = [])),
    params(("event_id" = EventId, Path)),
    request_body = UpdateEventRequest,
    responses((status = 200, body = Event), (status = 412, description = "Edited concurrently", body = Problem))
)]
pub(crate) async fn update_event(
    State(state): State<AppState>,
    principal: Principal,
    Path(event_id): Path<EventId>,
    Json(request): Json<UpdateEventRequest>,
) -> ApiResult<Json<Event>> {
    let current = writable_event(&state, &principal, event_id).await?;
    let event = Event {
        slug: Slug::new(request.slug)?,
        title: request.title,
        description: request.description,
        venue: request.venue,
        starts_at: request.starts_at,
        ends_at: request.ends_at,
        status: request.status,
        updated_at: state.now(),
        version: request.version + 1,
        ..current
    };
    event.validate()?;
    state.store().update_event(&event, request.version).await?;
    Ok(Json(event))
}

/// The sales of an event, with availability.
#[utoipa::path(
    get, path = "/v1/events/{event_id}/sales", tag = TAG,
    params(("event_id" = EventId, Path)),
    responses((status = 200, body = Vec<SaleDetail>), (status = 404, body = Problem))
)]
pub(crate) async fn list_sales(
    State(state): State<AppState>,
    principal: Option<Principal>,
    Path(event_id): Path<EventId>,
) -> ApiResult<Json<Vec<SaleDetail>>> {
    let event = visible_event(&state, principal.as_ref(), event_id).await?;
    let mut sales = Vec::new();
    for sale in state.store().list_sales(event.id).await? {
        sales.push(sale_detail(&state, sale).await?);
    }
    Ok(Json(sales))
}

fn sale_from(request: SaleRequest, current: &Sale) -> Sale {
    Sale {
        id: current.id,
        event_id: current.event_id,
        created_at: current.created_at,
        version: current.version,
        name: request.name,
        opens_at: request.opens_at,
        closes_at: request.closes_at,
        admission: request.admission,
        reservation_ttl_seconds: request.reservation_ttl_seconds,
        max_tickets_per_request: request.max_tickets_per_request,
        accepted_attestors: request.accepted_attestors,
        environment: request.environment,
    }
}

/// Create a sale for an event.
#[utoipa::path(
    post, path = "/v1/events/{event_id}/sales", tag = TAG,
    security(("bearer" = [])),
    params(("event_id" = EventId, Path)),
    request_body = SaleRequest,
    responses((status = 201, body = Sale), (status = 403, body = Problem))
)]
pub(crate) async fn create_sale(
    State(state): State<AppState>,
    principal: Principal,
    Path(event_id): Path<EventId>,
    Json(request): Json<SaleRequest>,
) -> ApiResult<(StatusCode, Json<Sale>)> {
    let event = writable_event(&state, &principal, event_id).await?;
    let blank = Sale {
        id: SaleId::generate(),
        event_id: event.id,
        name: String::new(),
        opens_at: request.opens_at,
        closes_at: request.closes_at,
        admission: request.admission,
        reservation_ttl_seconds: 0,
        max_tickets_per_request: 0,
        accepted_attestors: Vec::new(),
        environment: request.environment,
        created_at: state.now(),
        version: 1,
    };
    let sale = sale_from(request, &blank);
    sale.validate()?;
    state.store().insert_sale(&sale).await?;
    Ok((StatusCode::CREATED, Json(sale)))
}

/// A sale, with its ticket types and availability.
#[utoipa::path(
    get, path = "/v1/sales/{sale_id}", tag = TAG,
    params(("sale_id" = SaleId, Path)),
    responses((status = 200, body = SaleDetail), (status = 404, body = Problem))
)]
pub(crate) async fn get_sale(
    State(state): State<AppState>,
    principal: Option<Principal>,
    Path(sale_id): Path<SaleId>,
) -> ApiResult<Json<SaleDetail>> {
    let (sale, _) = visible_sale(&state, principal.as_ref(), sale_id).await?;
    Ok(Json(sale_detail(&state, sale).await?))
}

/// Edit a sale.
#[utoipa::path(
    put, path = "/v1/sales/{sale_id}", tag = TAG,
    security(("bearer" = [])),
    params(("sale_id" = SaleId, Path)),
    request_body = SaleRequest,
    responses((status = 200, body = Sale), (status = 412, body = Problem))
)]
pub(crate) async fn update_sale(
    State(state): State<AppState>,
    principal: Principal,
    Path(sale_id): Path<SaleId>,
    Json(request): Json<SaleRequest>,
) -> ApiResult<Json<Sale>> {
    let (current, _) = writable_sale(&state, &principal, sale_id).await?;
    let expected = version_of(request.version)?;
    let sale = Sale {
        version: expected + 1,
        ..sale_from(request, &current)
    };
    sale.validate()?;
    state.store().update_sale(&sale, expected).await?;
    Ok(Json(sale))
}

fn ticket_type_from(request: TicketTypeRequest, current: &TicketType) -> TicketType {
    TicketType {
        id: current.id,
        sale_id: current.sale_id,
        event_id: current.event_id,
        created_at: current.created_at,
        version: current.version,
        name: request.name,
        price: request.price,
        capacity: request.capacity,
        per_account_limit: request.per_account_limit,
        valid_from: request.valid_from,
        valid_until: request.valid_until,
        ticket_extensions: request.ticket_extensions,
    }
}

/// Add a ticket type to a sale.
#[utoipa::path(
    post, path = "/v1/sales/{sale_id}/ticket-types", tag = TAG,
    security(("bearer" = [])),
    params(("sale_id" = SaleId, Path)),
    request_body = TicketTypeRequest,
    responses((status = 201, body = TicketType), (status = 403, body = Problem))
)]
pub(crate) async fn create_ticket_type(
    State(state): State<AppState>,
    principal: Principal,
    Path(sale_id): Path<SaleId>,
    Json(request): Json<TicketTypeRequest>,
) -> ApiResult<(StatusCode, Json<TicketType>)> {
    let (sale, event) = writable_sale(&state, &principal, sale_id).await?;
    let blank = TicketType {
        id: TicketTypeId::generate(),
        sale_id: sale.id,
        event_id: event.id,
        name: String::new(),
        price: request.price,
        capacity: 0,
        per_account_limit: 0,
        valid_from: request.valid_from,
        valid_until: request.valid_until,
        ticket_extensions: BTreeMap::new(),
        created_at: state.now(),
        version: 1,
    };
    let ticket_type = ticket_type_from(request, &blank);
    ticket_type.validate()?;
    state.store().insert_ticket_type(&ticket_type).await?;
    Ok((StatusCode::CREATED, Json(ticket_type)))
}

/// Edit a ticket type. Capacity can drop no lower than what is held and sold.
#[utoipa::path(
    put, path = "/v1/ticket-types/{ticket_type_id}", tag = TAG,
    security(("bearer" = [])),
    params(("ticket_type_id" = TicketTypeId, Path)),
    request_body = TicketTypeRequest,
    responses((status = 200, body = TicketType), (status = 409, body = Problem), (status = 412, body = Problem))
)]
pub(crate) async fn update_ticket_type(
    State(state): State<AppState>,
    principal: Principal,
    Path(ticket_type_id): Path<TicketTypeId>,
    Json(request): Json<TicketTypeRequest>,
) -> ApiResult<Json<TicketType>> {
    let current = state
        .store()
        .ticket_type(ticket_type_id)
        .await?
        .ok_or_else(|| ApiError::not_found("ticket type"))?;
    writable_sale(&state, &principal, current.sale_id).await?;
    let expected = version_of(request.version)?;
    let ticket_type = TicketType {
        version: expected + 1,
        ..ticket_type_from(request, &current)
    };
    ticket_type.validate()?;
    state
        .store()
        .update_ticket_type(&ticket_type, expected)
        .await?;
    Ok(Json(ticket_type))
}

/// Your favourite events.
#[utoipa::path(
    get, path = "/v1/me/favorites", tag = TAG,
    security(("bearer" = [])),
    responses((status = 200, body = Vec<Event>))
)]
pub(crate) async fn list_favorites(
    State(state): State<AppState>,
    principal: Principal,
) -> ApiResult<Json<Vec<Event>>> {
    let account = principal.require_account()?;
    state.authorize(&principal, FAVORITES_MANAGE, Scope::Account(account))?;
    let mut events = Vec::new();
    for event_id in state.store().favorites(account).await? {
        if let Ok(event) = visible_event(&state, Some(&principal), event_id).await {
            events.push(event);
        }
    }
    Ok(Json(events))
}

/// Mark an event as a favourite.
#[utoipa::path(
    put, path = "/v1/me/favorites/{event_id}", tag = TAG,
    security(("bearer" = [])),
    params(("event_id" = EventId, Path)),
    responses((status = 204), (status = 404, body = Problem))
)]
pub(crate) async fn add_favorite(
    State(state): State<AppState>,
    principal: Principal,
    Path(event_id): Path<EventId>,
) -> ApiResult<StatusCode> {
    let account = principal.require_account()?;
    state.authorize(&principal, FAVORITES_MANAGE, Scope::Account(account))?;
    visible_event(&state, Some(&principal), event_id).await?;
    state
        .store()
        .add_favorite(account, event_id, state.now())
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Forget a favourite.
#[utoipa::path(
    delete, path = "/v1/me/favorites/{event_id}", tag = TAG,
    security(("bearer" = [])),
    params(("event_id" = EventId, Path)),
    responses((status = 204))
)]
pub(crate) async fn remove_favorite(
    State(state): State<AppState>,
    principal: Principal,
    Path(event_id): Path<EventId>,
) -> ApiResult<StatusCode> {
    let account = principal.require_account()?;
    state.authorize(&principal, FAVORITES_MANAGE, Scope::Account(account))?;
    state.store().remove_favorite(account, event_id).await?;
    Ok(StatusCode::NO_CONTENT)
}
