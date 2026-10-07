use async_trait::async_trait;
use kippu_domain::webhook::Webhook;
use kippu_domain::{OrganizationId, Timestamp, WebhookId};
use kippu_store::{Lease, StoreError, StoreResult, WebhookRun, WebhookStore};
use sqlx::types::Json;

use crate::convert::{WebhookRow, all, at, count, optional};
use crate::{PostgresStore, error};

const WEBHOOK_COLUMNS: &str = "id, organization_id, url, topics, active, delivered_through, \
     failures, last_error, next_attempt_at, created_at, version";

#[async_trait]
impl WebhookStore for PostgresStore {
    async fn insert_webhook(&self, webhook: &Webhook) -> StoreResult<()> {
        sqlx::query(
            "INSERT INTO webhooks (id, organization_id, url, topics, active, delivered_through,
                                   failures, last_error, next_attempt_at, lease_until,
                                   created_at, version)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, NULL, $10, $11)",
        )
        .bind(webhook.id.as_uuid())
        .bind(webhook.organization_id.map(|id| id.as_uuid()))
        .bind(&webhook.url)
        .bind(Json(&webhook.topics))
        .bind(webhook.active)
        .bind(webhook.delivered_through)
        .bind(count(webhook.failures))
        .bind(webhook.last_error.as_deref())
        .bind(at(webhook.next_attempt_at))
        .bind(at(webhook.created_at))
        .bind(webhook.version)
        .execute(&self.pool)
        .await
        .map_err(error)?;
        Ok(())
    }

    async fn webhook(&self, id: WebhookId) -> StoreResult<Option<Webhook>> {
        let row = sqlx::query_as::<_, WebhookRow>(sqlx::AssertSqlSafe(format!(
            "SELECT {WEBHOOK_COLUMNS} FROM webhooks WHERE id = $1"
        )))
        .bind(id.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(error)?;
        optional(row)
    }

    async fn list_webhooks(
        &self,
        organization: Option<OrganizationId>,
    ) -> StoreResult<Vec<Webhook>> {
        let rows = sqlx::query_as::<_, WebhookRow>(sqlx::AssertSqlSafe(format!(
            "SELECT {WEBHOOK_COLUMNS} FROM webhooks WHERE organization_id IS NOT DISTINCT FROM $1 ORDER BY id"
        )))
        .bind(organization.map(|id| id.as_uuid()))
        .fetch_all(&self.pool)
        .await
        .map_err(error)?;
        all(rows)
    }

    async fn update_webhook(&self, webhook: &Webhook, expected_version: i64) -> StoreResult<()> {
        let result = sqlx::query(
            "UPDATE webhooks SET url = $2, topics = $3, active = $4, version = $5
             WHERE id = $1 AND version = $6",
        )
        .bind(webhook.id.as_uuid())
        .bind(&webhook.url)
        .bind(Json(&webhook.topics))
        .bind(webhook.active)
        .bind(webhook.version)
        .bind(expected_version)
        .execute(&self.pool)
        .await
        .map_err(error)?;
        if result.rows_affected() == 1 {
            Ok(())
        } else {
            Err(StoreError::Conflict("version"))
        }
    }

    async fn delete_webhook(&self, id: WebhookId) -> StoreResult<bool> {
        let result = sqlx::query("DELETE FROM webhooks WHERE id = $1")
            .bind(id.as_uuid())
            .execute(&self.pool)
            .await
            .map_err(error)?;
        Ok(result.rows_affected() == 1)
    }

    async fn claim_webhook(&self, lease: Lease) -> StoreResult<Option<Webhook>> {
        let row = sqlx::query_as::<_, WebhookRow>(sqlx::AssertSqlSafe(format!(
            "UPDATE webhooks SET lease_until = $1
             WHERE id = (SELECT id FROM webhooks
                         WHERE active AND next_attempt_at <= $2
                           AND (lease_until IS NULL OR lease_until <= $2)
                           AND delivered_through < (SELECT COALESCE(MAX(sequence), 0) FROM outbox)
                         ORDER BY next_attempt_at LIMIT 1
                         FOR UPDATE SKIP LOCKED)
             RETURNING {WEBHOOK_COLUMNS}"
        )))
        .bind(at(lease.until))
        .bind(at(lease.now))
        .fetch_optional(&self.pool)
        .await
        .map_err(error)?;
        optional(row)
    }

    async fn finish_webhook_run(
        &self,
        id: WebhookId,
        lease_until: Timestamp,
        run: &WebhookRun,
    ) -> StoreResult<bool> {
        let result = sqlx::query(
            "UPDATE webhooks SET delivered_through = $3, failures = $4, last_error = $5,
                                 next_attempt_at = $6, lease_until = NULL
             WHERE id = $1 AND lease_until = $2",
        )
        .bind(id.as_uuid())
        .bind(at(lease_until))
        .bind(run.delivered_through)
        .bind(count(run.failures))
        .bind(run.last_error.as_deref())
        .bind(at(run.next_attempt_at))
        .execute(&self.pool)
        .await
        .map_err(error)?;
        Ok(result.rows_affected() == 1)
    }
}
