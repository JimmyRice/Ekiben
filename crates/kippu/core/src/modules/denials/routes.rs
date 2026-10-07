//! HTTP handlers: each turns a request into one [`service`](super::service) call and its
//! result into a response. Rules live in the service, not here.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use kippu_domain::denial::{Denial, DeniedTicket};
use kippu_domain::{DenialId, EventId, OrganizationId};

use super::dto::{CreateDenialRequest, DenialPatch};
use super::service::{self, AddedDenial};
use crate::app::AppState;
use crate::auth::Principal;
use crate::error::{ApiResult, Problem};
use crate::http::idempotency::IdempotentByDesign;
use crate::http::{Json, Listing, ListingTag, PageQuery};

const TAG: &str = "denials";

/// 201 for a new denial, 200 for one that was already there. Safe to repeat without the
/// idempotency middleware: the id is derived from organization, event and subject.
fn added(added: AddedDenial) -> Response {
    let status = if added.created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    let mut response = (status, Json(added.denial)).into_response();
    response.extensions_mut().insert(IdempotentByDesign);
    response
}

/// Refuse a ticket or an account entry to every event of an organization, present and
/// future. A denied account's purchase requests are rejected from then on (a reservation it
/// already holds can still be paid; those tickets are refused at the gate too). Adding the
/// same denial again answers 200 with the one already there.
#[utoipa::path(
    post, path = "/v1/organizations/{organization_id}/denials", tag = TAG,
    security(("bearer" = [])),
    params(("organization_id" = OrganizationId, Path)),
    request_body = CreateDenialRequest,
    responses(
        (status = 201, body = Denial),
        (status = 200, description = "The denial already existed", body = Denial),
        (status = 403, body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn create_organization_denial(
    State(state): State<AppState>,
    principal: Principal,
    Path(organization_id): Path<OrganizationId>,
    Json(request): Json<CreateDenialRequest>,
) -> ApiResult<Response> {
    Ok(added(
        service::create_organization_denial(&state, &principal, organization_id, request.into())
            .await?,
    ))
}

/// An organization's denials for all of its events, most recently added first.
#[utoipa::path(
    get, path = "/v1/organizations/{organization_id}/denials", tag = TAG,
    security(("bearer" = [])),
    params(("organization_id" = OrganizationId, Path), PageQuery),
    responses((status = 200, body = Listing<Denial>), (status = 403, body = Problem))
)]
pub(crate) async fn list_organization_denials(
    State(state): State<AppState>,
    principal: Principal,
    Path(organization_id): Path<OrganizationId>,
    Query(query): Query<PageQuery>,
) -> ApiResult<Json<Listing<Denial>>> {
    let page = query.page(ListingTag::ORGANIZATION_DENIALS)?;
    let denials = service::organization_denials(&state, &principal, organization_id, page).await?;
    Ok(Json(Listing::page(
        denials,
        ListingTag::ORGANIZATION_DENIALS,
        |denial| denial,
    )))
}

/// Refuse a ticket or an account entry to one event. A denied account's purchase requests
/// are rejected from then on (a reservation it already holds can still be paid; those tickets
/// are refused at the gate too). Adding the same denial again answers 200 with the one
/// already there.
#[utoipa::path(
    post, path = "/v1/events/{event_id}/denials", tag = TAG,
    security(("bearer" = [])),
    params(("event_id" = EventId, Path)),
    request_body = CreateDenialRequest,
    responses(
        (status = 201, body = Denial),
        (status = 200, description = "The denial already existed", body = Denial),
        (status = 403, body = Problem),
        (status = 404, body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn create_event_denial(
    State(state): State<AppState>,
    principal: Principal,
    Path(event_id): Path<EventId>,
    Json(request): Json<CreateDenialRequest>,
) -> ApiResult<Response> {
    Ok(added(
        service::create_event_denial(&state, &principal, event_id, request.into()).await?,
    ))
}

/// An event's own denials (not its organization's), most recently added first.
#[utoipa::path(
    get, path = "/v1/events/{event_id}/denials", tag = TAG,
    security(("bearer" = [])),
    params(("event_id" = EventId, Path), PageQuery),
    responses((status = 200, body = Listing<Denial>), (status = 403, body = Problem), (status = 404, body = Problem))
)]
pub(crate) async fn list_event_denials(
    State(state): State<AppState>,
    principal: Principal,
    Path(event_id): Path<EventId>,
    Query(query): Query<PageQuery>,
) -> ApiResult<Json<Listing<Denial>>> {
    let page = query.page(ListingTag::EVENT_DENIALS)?;
    let denials = service::event_denials(&state, &principal, event_id, page).await?;
    Ok(Json(Listing::page(
        denials,
        ListingTag::EVENT_DENIALS,
        |denial| denial,
    )))
}

/// A denial.
#[utoipa::path(
    get, path = "/v1/denials/{denial_id}", tag = TAG,
    security(("bearer" = [])),
    params(("denial_id" = DenialId, Path)),
    responses((status = 200, body = Denial), (status = 404, body = Problem))
)]
pub(crate) async fn get_denial(
    State(state): State<AppState>,
    principal: Principal,
    Path(denial_id): Path<DenialId>,
) -> ApiResult<Json<Denial>> {
    Ok(Json(service::denial(&state, &principal, denial_id).await?))
}

/// Change a denial's note.
#[utoipa::path(
    patch, path = "/v1/denials/{denial_id}", tag = TAG,
    security(("bearer" = [])),
    params(("denial_id" = DenialId, Path)),
    request_body(content((DenialPatch = "application/merge-patch+json"), (DenialPatch = "application/json"))),
    responses((status = 200, body = Denial), (status = 404, body = Problem), (status = 412, body = Problem))
)]
pub(crate) async fn patch_denial(
    State(state): State<AppState>,
    principal: Principal,
    Path(denial_id): Path<DenialId>,
    Json(patch): Json<DenialPatch>,
) -> ApiResult<Json<Denial>> {
    let version = patch.version;
    Ok(Json(
        service::update_denial(&state, &principal, denial_id, version, patch.into()).await?,
    ))
}

/// Lift a denial: its ticket, or its account's tickets, are admitted again unless revoked or
/// denied otherwise.
#[utoipa::path(
    delete, path = "/v1/denials/{denial_id}", tag = TAG,
    security(("bearer" = [])),
    params(("denial_id" = DenialId, Path)),
    responses((status = 204), (status = 404, body = Problem))
)]
pub(crate) async fn delete_denial(
    State(state): State<AppState>,
    principal: Principal,
    Path(denial_id): Path<DenialId>,
) -> ApiResult<StatusCode> {
    service::delete_denial(&state, &principal, denial_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// For gates: the tickets of an event to refuse although their signature is valid, in
/// ticket id order — tickets revoked by a refund, tickets denied, and every ticket of a denied
/// account, by the event's list or its organization's. Compare a verified ticket's
/// `ticket_id` with it.
#[utoipa::path(
    get, path = "/v1/events/{event_id}/denied-tickets", tag = TAG,
    security(("bearer" = [])),
    params(("event_id" = EventId, Path), PageQuery),
    responses((status = 200, body = Listing<DeniedTicket>), (status = 403, body = Problem), (status = 404, body = Problem))
)]
pub(crate) async fn denied_tickets(
    State(state): State<AppState>,
    principal: Principal,
    Path(event_id): Path<EventId>,
    Query(query): Query<PageQuery>,
) -> ApiResult<Json<Listing<DeniedTicket>>> {
    let page = query.page(ListingTag::DENIED_TICKETS)?;
    let tickets = service::denied_tickets(&state, &principal, event_id, page).await?;
    Ok(Json(Listing::page(
        tickets,
        ListingTag::DENIED_TICKETS,
        |denied| denied,
    )))
}
