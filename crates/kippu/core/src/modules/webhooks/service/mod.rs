//! Webhook use cases, independent of HTTP: who may register and manage which webhooks, and
//! the rules a webhook's settings must follow. Delivering events is `dispatch.rs`.

mod dispatch;

pub(crate) use dispatch::deliver;

use kippu_domain::webhook::Webhook;
use kippu_domain::{OrganizationId, ValidationError, WebhookId};

use super::delivery::refusal;
use super::signature::key_id;
use crate::app::AppState;
use crate::auth::{Principal, Scope};
use crate::error::{ApiError, ApiResult, StatusCode};
use crate::modules::webhooks::permissions::{WEBHOOKS_MANAGE, WEBHOOKS_MANAGE_ALL};

/// A webhook to register.
#[derive(Debug, Clone)]
pub struct NewWebhook {
    /// Where to POST events.
    pub url: String,
    /// Topics to receive; empty means all of them.
    pub topics: Vec<String>,
}

/// Changes to a webhook. `None` keeps the stored value.
#[derive(Debug, Clone, Default)]
pub struct WebhookChanges {
    /// Where to POST events.
    pub url: Option<String>,
    /// Topics to receive, replaced as a whole.
    pub topics: Option<Vec<String>>,
    /// Pause (`false`) or resume (`true`) delivery.
    pub active: Option<bool>,
}

/// A key webhook deliveries are signed with.
#[derive(Debug, Clone)]
pub struct SigningKey {
    /// The id in `Kippu-Signature`'s `key=`.
    pub key_id: String,
    /// The Ed25519 public key.
    pub public_key: [u8; 32],
}

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

/// Checks that the caller may manage the webhooks of `owner`: an organization, or `None` for
/// the global ones that receive every event.
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

/// A webhook the caller may manage. Others' webhooks are reported as missing.
pub async fn webhook(state: &AppState, principal: &Principal, id: WebhookId) -> ApiResult<Webhook> {
    let webhook = state
        .store()
        .webhook(id)
        .await?
        .ok_or_else(|| ApiError::not_found("webhook"))?;
    authorize(state, principal, webhook.organization_id)
        .map_err(|_| ApiError::not_found("webhook"))?;
    Ok(webhook)
}

/// Registers a webhook for an organization's events, or with `owner` `None` for every event
/// of the deployment. It receives events from now on, not those recorded before.
pub async fn create_webhook(
    state: &AppState,
    principal: &Principal,
    owner: Option<OrganizationId>,
    new: NewWebhook,
) -> ApiResult<Webhook> {
    authorize(state, principal, owner)?;
    if let Some(organization) = owner {
        state
            .store()
            .organization(organization)
            .await?
            .ok_or_else(|| ApiError::not_found("organization"))?;
    }
    if state.webhook_key().is_none() {
        return Err(not_configured());
    }
    let now = state.now();
    let webhook = Webhook {
        id: WebhookId::generate(),
        organization_id: owner,
        url: new.url,
        topics: new.topics,
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
    state.audit(principal, "webhook.create", webhook.id).await?;
    Ok(webhook)
}

/// The webhooks of an organization, or with `owner` `None` the global ones.
pub async fn webhooks(
    state: &AppState,
    principal: &Principal,
    owner: Option<OrganizationId>,
) -> ApiResult<Vec<Webhook>> {
    authorize(state, principal, owner)?;
    Ok(state.store().list_webhooks(owner).await?)
}

/// Changes a webhook's URL or topics, or pauses and resumes it. `expected_version` is the
/// version the caller read.
pub async fn update_webhook(
    state: &AppState,
    principal: &Principal,
    id: WebhookId,
    expected_version: i64,
    changes: WebhookChanges,
) -> ApiResult<Webhook> {
    let current = webhook(state, principal, id).await?;
    if current.version != expected_version {
        return Err(ApiError::stale_version());
    }
    let updated = Webhook {
        url: changes.url.unwrap_or_else(|| current.url.clone()),
        topics: changes.topics.unwrap_or_else(|| current.topics.clone()),
        active: changes.active.unwrap_or(current.active),
        version: expected_version + 1,
        ..current
    };
    check(state, &updated)?;
    state
        .store()
        .update_webhook(&updated, expected_version)
        .await?;
    state.audit(principal, "webhook.update", updated.id).await?;
    // Progress may have moved since it was read; answer with what is stored.
    webhook(state, principal, id).await
}

/// Deletes a webhook. Deliveries in flight may still arrive.
pub async fn delete_webhook(
    state: &AppState,
    principal: &Principal,
    id: WebhookId,
) -> ApiResult<()> {
    webhook(state, principal, id).await?;
    state.store().delete_webhook(id).await?;
    state.audit(principal, "webhook.delete", id).await
}

/// The keys webhook deliveries are signed with, or 501 if the deployment has none.
pub fn signing_keys(state: &AppState) -> ApiResult<Vec<SigningKey>> {
    let key = state
        .webhook_key()
        .ok_or_else(not_configured)?
        .verifying_key();
    Ok(vec![SigningKey {
        key_id: key_id(&key),
        public_key: key.to_bytes(),
    }])
}
