use axum::extract::{Path, State};
use axum::http::StatusCode;
use kippu_domain::webhook::Webhook;
use kippu_domain::{OrganizationId, ValidationError, WebhookId};

use super::delivery::refusal;
use super::dto::{CreateWebhookRequest, WebhookKey, WebhookKeys, WebhookPatch};
use super::permissions::{WEBHOOKS_MANAGE, WEBHOOKS_MANAGE_ALL};
use super::signature::key_id;
use crate::app::AppState;
use crate::auth::{Principal, Scope};
use crate::error::{ApiError, ApiResult, Problem};
use crate::http::Json;
use crate::keys::encode_key;

const TAG: &str = "webhooks";

fn not_configured() -> ApiError {
    ApiError::new(
        StatusCode::NOT_IMPLEMENTED,
        "webhooks-not-configured",
        "this deployment has no webhook signing key (keys.webhook_signing_key)",
    )
}

/// Checks a webhook's settings against the domain rules and the deployment's URL policy.
fn check(state: &AppState, webhook: &Webhook) -> ApiResult<()> {
    webhook.validate()?;
    if let Some(reason) = refusal(&webhook.url, &state.config().webhooks) {
        return Err(ValidationError::new("url", reason).into());
    }
    Ok(())
}

/// The scope a webhook is managed in: its organization, or everything for a global one.
fn authorize(
    state: &AppState,
    principal: &Principal,
    owner: Option<OrganizationId>,
) -> ApiResult<()> {
    match owner {
        Some(organization) => state.authorize(
            principal,
            WEBHOOKS_MANAGE,
            Scope::Organization(organization),
        ),
        None => state.authorize(principal, WEBHOOKS_MANAGE_ALL, Scope::Global),
    }
}

async fn create(
    state: &AppState,
    owner: Option<OrganizationId>,
    request: CreateWebhookRequest,
) -> ApiResult<Webhook> {
    if state.webhook_key().is_none() {
        return Err(not_configured());
    }
    let now = state.now();
    let webhook = Webhook {
        id: WebhookId::generate(),
        organization_id: owner,
        url: request.url,
        topics: request.topics,
        active: true,
        // From now on: events recorded before registration are not delivered.
        delivered_through: state.store().latest_sequence().await?,
        failures: 0,
        last_error: None,
        next_attempt_at: now,
        created_at: now,
        version: 1,
    };
    check(state, &webhook)?;
    state.store().insert_webhook(&webhook).await?;
    Ok(webhook)
}

/// A webhook the caller may manage. Others' webhooks are reported as missing.
async fn manageable(state: &AppState, principal: &Principal, id: WebhookId) -> ApiResult<Webhook> {
    let webhook = state
        .store()
        .webhook(id)
        .await?
        .ok_or_else(|| ApiError::not_found("webhook"))?;
    authorize(state, principal, webhook.organization_id)
        .map_err(|_| ApiError::not_found("webhook"))?;
    Ok(webhook)
}

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
    authorize(&state, &principal, Some(organization_id))?;
    state
        .store()
        .organization(organization_id)
        .await?
        .ok_or_else(|| ApiError::not_found("organization"))?;
    let webhook = create(&state, Some(organization_id), request).await?;
    state
        .audit(&principal, "webhook.create", webhook.id)
        .await?;
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
    authorize(&state, &principal, Some(organization_id))?;
    Ok(Json(
        state.store().list_webhooks(Some(organization_id)).await?,
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
    authorize(&state, &principal, None)?;
    let webhook = create(&state, None, request).await?;
    state
        .audit(&principal, "webhook.create", webhook.id)
        .await?;
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
    authorize(&state, &principal, None)?;
    Ok(Json(state.store().list_webhooks(None).await?))
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
    Ok(Json(manageable(&state, &principal, webhook_id).await?))
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
    let current = manageable(&state, &principal, webhook_id).await?;
    if current.version != patch.version {
        return Err(ApiError::stale_version());
    }
    let webhook = Webhook {
        url: patch.url.unwrap_or_else(|| current.url.clone()),
        topics: patch.topics.unwrap_or_else(|| current.topics.clone()),
        active: patch.active.unwrap_or(current.active),
        version: patch.version + 1,
        ..current
    };
    check(&state, &webhook)?;
    state
        .store()
        .update_webhook(&webhook, patch.version)
        .await?;
    state
        .audit(&principal, "webhook.update", webhook.id)
        .await?;
    // Progress may have moved since it was read; answer with what is stored.
    Ok(Json(manageable(&state, &principal, webhook_id).await?))
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
    manageable(&state, &principal, webhook_id).await?;
    state.store().delete_webhook(webhook_id).await?;
    state
        .audit(&principal, "webhook.delete", webhook_id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The keys webhook deliveries are signed with.
#[utoipa::path(
    get, path = "/.well-known/kippu/webhook-keys", tag = TAG,
    responses((status = 200, body = WebhookKeys), (status = 501, body = Problem))
)]
pub(crate) async fn webhook_keys(State(state): State<AppState>) -> ApiResult<Json<WebhookKeys>> {
    let key = state
        .webhook_key()
        .ok_or_else(not_configured)?
        .verifying_key();
    Ok(Json(WebhookKeys {
        keys: vec![WebhookKey {
            key_id: key_id(&key),
            algorithm: "Ed25519",
            public_key: encode_key(key.as_bytes()),
        }],
    }))
}
