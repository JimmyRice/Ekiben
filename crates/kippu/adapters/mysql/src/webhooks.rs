use async_trait::async_trait;
use kippu_domain::webhook::Webhook;
use kippu_domain::{OrganizationId, Timestamp, WebhookId};
use kippu_store::{Lease, StoreError, StoreResult, WebhookRun, WebhookStore};
use uuid::Uuid;

use crate::convert::{WebhookRow, all, json, micros, optional};
use crate::{MySqlStore, error};

const WEBHOOK_COLUMNS: &str = "id, organization_id, url, topics, active, delivered_through, \
     failures, last_error, next_attempt_at, created_at, version";

#[async_trait]
impl WebhookStore for MySqlStore {
    async fn insert_webhook(&self, webhook: &Webhook) -> StoreResult<()> {
        sqlx::query(
            "INSERT INTO webhooks (id, organization_id, url, topics, active, delivered_through,
                                   failures, last_error, next_attempt_at, lease_until,
                                   created_at, version)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, ?, ?)",
        )
        .bind(webhook.id.as_uuid())
        .bind(webhook.organization_id.map(|id| id.as_uuid()))
        .bind(&webhook.url)
        .bind(json(&webhook.topics)?)
        .bind(webhook.active)
        .bind(webhook.delivered_through)
        .bind(webhook.failures)
        .bind(webhook.last_error.as_deref())
        .bind(micros(webhook.next_attempt_at))
        .bind(micros(webhook.created_at))
        .bind(webhook.version)
        .execute(&self.pool)
        .await
        .map_err(error)?;
        Ok(())
    }

    async fn webhook(&self, id: WebhookId) -> StoreResult<Option<Webhook>> {
        let row = sqlx::query_as::<_, WebhookRow>(sqlx::AssertSqlSafe(format!(
            "SELECT {WEBHOOK_COLUMNS} FROM webhooks WHERE id = ?"
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
            "SELECT {WEBHOOK_COLUMNS} FROM webhooks WHERE organization_id <=> ? ORDER BY id"
        )))
        .bind(organization.map(|id| id.as_uuid()))
        .fetch_all(&self.pool)
        .await
        .map_err(error)?;
        all(rows)
    }

    async fn update_webhook(&self, webhook: &Webhook, expected_version: i64) -> StoreResult<()> {
        let result = sqlx::query(
            "UPDATE webhooks SET url = ?, topics = ?, active = ?, version = ?
             WHERE id = ? AND version = ?",
        )
        .bind(&webhook.url)
        .bind(json(&webhook.topics)?)
        .bind(webhook.active)
        .bind(webhook.version)
        .bind(webhook.id.as_uuid())
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
        let result = sqlx::query("DELETE FROM webhooks WHERE id = ?")
            .bind(id.as_uuid())
            .execute(&self.pool)
            .await
            .map_err(error)?;
        Ok(result.rows_affected() == 1)
    }

    async fn claim_webhook(&self, lease: Lease) -> StoreResult<Option<Webhook>> {
        // No UPDATE … RETURNING: lock one due webhook, lease it, read it back. The due index
        // drives the scan so that only the row returned is locked.
        let mut tx = self.pool.begin().await.map_err(error)?;
        let id = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM webhooks FORCE INDEX (webhooks_due)
             WHERE active = 1 AND next_attempt_at <= ?
               AND (lease_until IS NULL OR lease_until <= ?)
               AND delivered_through < (SELECT COALESCE(MAX(sequence), 0) FROM outbox)
             ORDER BY next_attempt_at LIMIT 1
             FOR UPDATE SKIP LOCKED",
        )
        .bind(micros(lease.now))
        .bind(micros(lease.now))
        .fetch_optional(&mut *tx)
        .await
        .map_err(error)?;
        let Some(id) = id else {
            return Ok(None);
        };
        sqlx::query("UPDATE webhooks SET lease_until = ? WHERE id = ?")
            .bind(micros(lease.until))
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(error)?;
        let row = sqlx::query_as::<_, WebhookRow>(sqlx::AssertSqlSafe(format!(
            "SELECT {WEBHOOK_COLUMNS} FROM webhooks WHERE id = ?"
        )))
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .map_err(error)?;
        tx.commit().await.map_err(error)?;
        Ok(Some(row.try_into()?))
    }

    async fn finish_webhook_run(
        &self,
        id: WebhookId,
        lease_until: Timestamp,
        run: &WebhookRun,
    ) -> StoreResult<bool> {
        let result = sqlx::query(
            "UPDATE webhooks SET delivered_through = ?, failures = ?, last_error = ?,
                                 next_attempt_at = ?, lease_until = NULL
             WHERE id = ? AND lease_until = ?",
        )
        .bind(run.delivered_through)
        .bind(run.failures)
        .bind(run.last_error.as_deref())
        .bind(micros(run.next_attempt_at))
        .bind(id.as_uuid())
        .bind(micros(lease_until))
        .execute(&self.pool)
        .await
        .map_err(error)?;
        Ok(result.rows_affected() == 1)
    }
}
