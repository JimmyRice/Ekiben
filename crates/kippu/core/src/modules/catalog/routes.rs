//! HTTP handlers: each turns a request into one [`service`](super::service) call and its
//! result into a response. Rules about who may do what live in the service, not here.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use kippu_domain::catalog::{Event, EventSummary, Sale, TicketType};
use kippu_domain::{EventId, OrganizationId, SaleId, TicketTypeId};

use super::dto::{
    CreateEventRequest, EventPatch, SaleDetail, SalePatch, SaleRequest, TicketTypePatch,
    TicketTypeRequest, UpdateEventRequest, required_version,
};
use super::service;
use crate::app::AppState;
use crate::auth::Principal;
use crate::error::{ApiResult, Problem};
use crate::http::{Json, PageQuery};

const TAG: &str = "catalog";

/// Published events, for everyone. Listings leave out each event's `content`.
#[utoipa::path(
    get, path = "/v1/events", tag = TAG,
    params(PageQuery),
    responses((status = 200, body = Vec<EventSummary>))
)]
pub(crate) async fn list_events(
    State(state): State<AppState>,
    Query(query): Query<PageQuery>,
) -> ApiResult<Json<Vec<EventSummary>>> {
    Ok(Json(service::public_events(&state, query.page()).await?))
}

/// All events of an organization, drafts included.
#[utoipa::path(
    get, path = "/v1/organizations/{organization_id}/events", tag = TAG,
    security(("bearer" = [])),
    params(("organization_id" = OrganizationId, Path), PageQuery),
    responses((status = 200, body = Vec<EventSummary>), (status = 403, body = Problem))
)]
pub(crate) async fn list_organization_events(
    State(state): State<AppState>,
    principal: Principal,
    Path(organization_id): Path<OrganizationId>,
    Query(query): Query<PageQuery>,
) -> ApiResult<Json<Vec<EventSummary>>> {
    Ok(Json(
        service::organization_events(&state, &principal, organization_id, query.page()).await?,
    ))
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
    let event = service::create_event(&state, &principal, organization_id, request.into()).await?;
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
        service::visible_event(&state, principal.as_ref(), event_id).await?,
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
    let version = request.version;
    Ok(Json(
        service::update_event(&state, &principal, event_id, version, request.into()).await?,
    ))
}

/// Change some of an event's fields, including publishing or cancelling it.
#[utoipa::path(
    patch, path = "/v1/events/{event_id}", tag = TAG,
    security(("bearer" = [])),
    params(("event_id" = EventId, Path)),
    request_body(content((EventPatch = "application/merge-patch+json"), (EventPatch = "application/json"))),
    responses((status = 200, body = Event), (status = 412, description = "Edited concurrently", body = Problem))
)]
pub(crate) async fn patch_event(
    State(state): State<AppState>,
    principal: Principal,
    Path(event_id): Path<EventId>,
    Json(patch): Json<EventPatch>,
) -> ApiResult<Json<Event>> {
    let version = patch.version;
    Ok(Json(
        service::update_event(&state, &principal, event_id, version, patch.into()).await?,
    ))
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
    let sales = service::event_sales(&state, principal.as_ref(), event_id).await?;
    Ok(Json(sales.into_iter().map(SaleDetail::from).collect()))
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
    let sale = service::create_sale(&state, &principal, event_id, request.into()).await?;
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
    let offer = service::sale_offer(&state, principal.as_ref(), sale_id).await?;
    Ok(Json(offer.into()))
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
    let version = required_version(request.version)?;
    let settings = service::SaleSettings::from(request);
    Ok(Json(
        service::update_sale(&state, &principal, sale_id, version, settings.into()).await?,
    ))
}

/// Change some of a sale's settings.
#[utoipa::path(
    patch, path = "/v1/sales/{sale_id}", tag = TAG,
    security(("bearer" = [])),
    params(("sale_id" = SaleId, Path)),
    request_body(content((SalePatch = "application/merge-patch+json"), (SalePatch = "application/json"))),
    responses((status = 200, body = Sale), (status = 412, body = Problem))
)]
pub(crate) async fn patch_sale(
    State(state): State<AppState>,
    principal: Principal,
    Path(sale_id): Path<SaleId>,
    Json(patch): Json<SalePatch>,
) -> ApiResult<Json<Sale>> {
    let version = patch.version;
    Ok(Json(
        service::update_sale(&state, &principal, sale_id, version, patch.into()).await?,
    ))
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
    let ticket_type =
        service::create_ticket_type(&state, &principal, sale_id, request.into()).await?;
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
    let version = required_version(request.version)?;
    let settings = service::TicketTypeSettings::from(request);
    Ok(Json(
        service::update_ticket_type(&state, &principal, ticket_type_id, version, settings.into())
            .await?,
    ))
}

/// Change some of a ticket type's settings. Capacity can drop no lower than what is held and
/// sold.
#[utoipa::path(
    patch, path = "/v1/ticket-types/{ticket_type_id}", tag = TAG,
    security(("bearer" = [])),
    params(("ticket_type_id" = TicketTypeId, Path)),
    request_body(content((TicketTypePatch = "application/merge-patch+json"), (TicketTypePatch = "application/json"))),
    responses((status = 200, body = TicketType), (status = 409, body = Problem), (status = 412, body = Problem))
)]
pub(crate) async fn patch_ticket_type(
    State(state): State<AppState>,
    principal: Principal,
    Path(ticket_type_id): Path<TicketTypeId>,
    Json(patch): Json<TicketTypePatch>,
) -> ApiResult<Json<TicketType>> {
    let version = patch.version;
    Ok(Json(
        service::update_ticket_type(&state, &principal, ticket_type_id, version, patch.into())
            .await?,
    ))
}

/// Your favourite events.
#[utoipa::path(
    get, path = "/v1/me/favorites", tag = TAG,
    security(("bearer" = [])),
    responses((status = 200, body = Vec<EventSummary>))
)]
pub(crate) async fn list_favorites(
    State(state): State<AppState>,
    principal: Principal,
) -> ApiResult<Json<Vec<EventSummary>>> {
    Ok(Json(service::favorites(&state, &principal).await?))
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
    service::add_favorite(&state, &principal, event_id).await?;
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
    service::remove_favorite(&state, &principal, event_id).await?;
    Ok(StatusCode::NO_CONTENT)
}
