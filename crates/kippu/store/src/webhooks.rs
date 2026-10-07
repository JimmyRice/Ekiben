use async_trait::async_trait;
use kippu_domain::webhook::Webhook;
use kippu_domain::{OrganizationId, Timestamp, WebhookId};

use crate::{Lease, StoreResult};

/// How one delivery run ended, written back by [`WebhookStore::finish_webhook_run`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookRun {
    /// The outbox sequence delivered (or passed over) through.
    pub delivered_through: i64,
    /// Consecutive failures, 0 after a run that ended well.
    pub failures: u32,
    /// What went wrong, if the run ended in a failure.
    pub last_error: Option<String>,
    /// When to run next.
    pub next_attempt_at: Timestamp,
}

/// Webhook registrations and their delivery progress.
#[async_trait]
pub trait WebhookStore {
    /// Registers a webhook.
    async fn insert_webhook(&self, webhook: &Webhook) -> StoreResult<()>;

    /// Looks a webhook up by id.
    async fn webhook(&self, id: WebhookId) -> StoreResult<Option<Webhook>>;

    /// The webhooks of an organization, or the global ones (`None`), in id order.
    async fn list_webhooks(
        &self,
        organization: Option<OrganizationId>,
    ) -> StoreResult<Vec<Webhook>>;

    /// Replaces a webhook's settings (`url`, `topics`, `active`) if its stored version is
    /// still `expected_version`; delivery progress is left alone. Fails with
    /// `Conflict("version")` otherwise.
    async fn update_webhook(&self, webhook: &Webhook, expected_version: i64) -> StoreResult<()>;

    /// Deletes a webhook. Returns whether it existed.
    async fn delete_webhook(&self, id: WebhookId) -> StoreResult<bool>;

    /// Claims one active webhook that is due (`next_attempt_at` at or before `lease.now`),
    /// has events it has not been offered (`delivered_through` below the outbox's latest
    /// sequence) and is not leased, and leases it until `lease.until`.
    ///
    /// **Contract:** while a lease is live, no other caller can claim the same webhook, so
    /// its events are delivered by one worker at a time, in order.
    async fn claim_webhook(&self, lease: Lease) -> StoreResult<Option<Webhook>>;

    /// Records how a run ended and releases the lease — only if the webhook is still leased
    /// until `lease_until` (the claimer's lease). Returns whether it was recorded; a worker
    /// whose lease lapsed and was taken over records nothing.
    async fn finish_webhook_run(
        &self,
        id: WebhookId,
        lease_until: Timestamp,
        run: &WebhookRun,
    ) -> StoreResult<bool>;
}
