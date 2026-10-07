//! HTTP handlers: each turns a request into one [`service`](super::service) call and its
//! result into a response. Rules live in the service, not here.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use kippu_domain::webhook::Webhook;
use kippu_domain::{OrganizationId, WebhookId};

use super::dto::{CreateWebhookRequest, WebhookKey, WebhookKeys, WebhookPatch};
use super::service;
use crate::app::AppState;
use crate::auth::Principal;
use crate::error::{ApiResult, Problem};
use crate::http::Json;

const TAG: &str = "webhooks";

/// Register a webhook for an organization's events.
#[utoipa::path(
    post, path = "/v1/organizations/{organization_id}/webhooks", tag = TAG,
    security(("bearer" = [])),
    params(("organization_id" = OrganizationId, Path)),
    request_body = CreateWebhookRequest,
    responses(
        (status = 201, body = Webhook),
        (status = 403, body = Problem),
        (status = 422, body = Problem),
        (status = 501, description = "Webhooks are not configured", body = Problem)
    )
)]
pub(crate) async fn create_organization_webhook(
    State(state): State<AppState>,
    principal: Principal,
    Path(organization_id): Path<OrganizationId>,
    Json(request): Json<CreateWebhookRequest>,
) -> ApiResult<(StatusCode, Json<Webhook>)> {
    let webhook =
        service::create_webhook(&state, &principal, Some(organization_id), request.into()).await?;
    Ok((StatusCode::CREATED, Json(webhook)))
}

/// An organization's webhooks.
#[utoipa::path(
    get, path = "/v1/organizations/{organization_id}/webhooks", tag = TAG,
    security(("bearer" = [])),
    params(("organization_id" = OrganizationId, Path)),
    responses((status = 200, body = Vec<Webhook>), (status = 403, body = Problem))
)]
pub(crate) async fn list_organization_webhooks(
    State(state): State<AppState>,
    principal: Principal,
    Path(organization_id): Path<OrganizationId>,
) -> ApiResult<Json<Vec<Webhook>>> {
    Ok(Json(
        service::webhooks(&state, &principal, Some(organization_id)).await?,
    ))
}

/// Register a webhook for every event of the deployment.
#[utoipa::path(
    post, path = "/v1/admin/webhooks", tag = TAG,
    security(("bearer" = [])),
    request_body = CreateWebhookRequest,
    responses(
        (status = 201, body = Webhook),
        (status = 403, body = Problem),
        (status = 422, body = Problem),
        (status = 501, description = "Webhooks are not configured", body = Problem)
    )
)]
pub(crate) async fn create_global_webhook(
    State(state): State<AppState>,
    principal: Principal,
    Json(request): Json<CreateWebhookRequest>,
) -> ApiResult<(StatusCode, Json<Webhook>)> {
    let webhook = service::create_webhook(&state, &principal, None, request.into()).await?;
    Ok((StatusCode::CREATED, Json(webhook)))
}

/// The webhooks that receive every event.
#[utoipa::path(
    get, path = "/v1/admin/webhooks", tag = TAG,
    security(("bearer" = [])),
    responses((status = 200, body = Vec<Webhook>), (status = 403, body = Problem))
)]
pub(crate) async fn list_global_webhooks(
    State(state): State<AppState>,
    principal: Principal,
) -> ApiResult<Json<Vec<Webhook>>> {
    Ok(Json(service::webhooks(&state, &principal, None).await?))
}

/// A webhook, with its delivery progress.
#[utoipa::path(
    get, path = "/v1/webhooks/{webhook_id}", tag = TAG,
    security(("bearer" = [])),
    params(("webhook_id" = WebhookId, Path)),
    responses((status = 200, body = Webhook), (status = 404, body = Problem))
)]
pub(crate) async fn get_webhook(
    State(state): State<AppState>,
    principal: Principal,
    Path(webhook_id): Path<WebhookId>,
) -> ApiResult<Json<Webhook>> {
    Ok(Json(
        service::webhook(&state, &principal, webhook_id).await?,
    ))
}

/// Change a webhook's URL or topics, or pause and resume it.
#[utoipa::path(
    patch, path = "/v1/webhooks/{webhook_id}", tag = TAG,
    security(("bearer" = [])),
    params(("webhook_id" = WebhookId, Path)),
    request_body(content((WebhookPatch = "application/merge-patch+json"), (WebhookPatch = "application/json"))),
    responses((status = 200, body = Webhook), (status = 404, body = Problem), (status = 412, body = Problem))
)]
pub(crate) async fn patch_webhook(
    State(state): State<AppState>,
    principal: Principal,
    Path(webhook_id): Path<WebhookId>,
    Json(patch): Json<WebhookPatch>,
) -> ApiResult<Json<Webhook>> {
    let version = patch.version;
    Ok(Json(
        service::update_webhook(&state, &principal, webhook_id, version, patch.into()).await?,
    ))
}

/// Delete a webhook. Deliveries in flight may still arrive.
#[utoipa::path(
    delete, path = "/v1/webhooks/{webhook_id}", tag = TAG,
    security(("bearer" = [])),
    params(("webhook_id" = WebhookId, Path)),
    responses((status = 204), (status = 404, body = Problem))
)]
pub(crate) async fn delete_webhook(
    State(state): State<AppState>,
    principal: Principal,
    Path(webhook_id): Path<WebhookId>,
) -> ApiResult<StatusCode> {
    service::delete_webhook(&state, &principal, webhook_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The keys webhook deliveries are signed with.
#[utoipa::path(
    get, path = "/.well-known/kippu/webhook-keys", tag = TAG,
    responses((status = 200, body = WebhookKeys), (status = 501, body = Problem))
)]
pub(crate) async fn webhook_keys(State(state): State<AppState>) -> ApiResult<Json<WebhookKeys>> {
    let keys = service::signing_keys(&state)?;
    Ok(Json(WebhookKeys {
        keys: keys.into_iter().map(WebhookKey::from).collect(),
    }))
}
