use async_trait::async_trait;
use kippu_domain::denial::{Denial, DeniedTicket};
use kippu_domain::{AccountId, DenialId, EventId, OrganizationId};
use kippu_store::{DenialStore, Insertion, Keyset, PageRequest, StoreError, StoreResult};
use uuid::Uuid;

use crate::convert::{DenialRow, DeniedTicketRow, all, micros, optional};
use crate::{MySqlStore, error, is_duplicate};

const DENIAL_COLUMNS: &str = "SELECT id, organization_id, event_id, subject_kind, subject_id, \
     note, created_at, updated_at, version FROM denials";

#[async_trait]
impl DenialStore for MySqlStore {
    async fn insert_denial(&self, denial: &Denial) -> StoreResult<Insertion<Denial>> {
        let inserted = sqlx::query(
            "INSERT INTO denials (id, organization_id, event_id, subject_kind, subject_id, note,
                                  created_at, updated_at, version)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(denial.id.as_uuid())
        .bind(denial.organization_id.as_uuid())
        .bind(denial.event_id.map(|id| id.as_uuid()))
        .bind(denial.subject.kind())
        .bind(denial.subject.id())
        .bind(&denial.note)
        .bind(micros(denial.created_at))
        .bind(micros(denial.updated_at))
        .bind(denial.version)
        .execute(&self.pool)
        .await;
        match inserted {
            Ok(_) => return Ok(Insertion::Inserted),
            Err(failure) if is_duplicate(&failure) => {}
            Err(failure) => return Err(error(failure)),
        }
        let existing = sqlx::query_as::<_, DenialRow>(sqlx::AssertSqlSafe(format!(
            "{DENIAL_COLUMNS} WHERE id = ?"
        )))
        .bind(denial.id.as_uuid())
        .fetch_one(&self.pool)
        .await
        .map_err(error)?;
        Ok(Insertion::Existing(existing.try_into()?))
    }

    async fn denial(&self, id: DenialId) -> StoreResult<Option<Denial>> {
        let row = sqlx::query_as::<_, DenialRow>(sqlx::AssertSqlSafe(format!(
            "{DENIAL_COLUMNS} WHERE id = ?"
        )))
        .bind(id.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(error)?;
        optional(row)
    }

    async fn update_denial(&self, denial: &Denial, expected_version: i64) -> StoreResult<()> {
        let result = sqlx::query(
            "UPDATE denials SET note = ?, updated_at = ?, version = ?
             WHERE id = ? AND version = ?",
        )
        .bind(&denial.note)
        .bind(micros(denial.updated_at))
        .bind(denial.version)
        .bind(denial.id.as_uuid())
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

    async fn delete_denial(&self, id: DenialId) -> StoreResult<bool> {
        let result = sqlx::query("DELETE FROM denials WHERE id = ?")
            .bind(id.as_uuid())
            .execute(&self.pool)
            .await
            .map_err(error)?;
        Ok(result.rows_affected() == 1)
    }

    async fn organization_denials(
        &self,
        organization: OrganizationId,
        page: PageRequest<Keyset>,
    ) -> StoreResult<Vec<Denial>> {
        let after_at = page.after.map(|after| micros(after.at));
        let rows = sqlx::query_as::<_, DenialRow>(sqlx::AssertSqlSafe(format!(
            "{DENIAL_COLUMNS} WHERE organization_id = ? AND event_id IS NULL
               AND (? IS NULL OR created_at < ? OR (created_at = ? AND id > ?))
             ORDER BY created_at DESC, id LIMIT ?"
        )))
        .bind(organization.as_uuid())
        .bind(after_at)
        .bind(after_at)
        .bind(after_at)
        .bind(page.after.map(|after| after.id))
        .bind(page.limit)
        .fetch_all(&self.pool)
        .await
        .map_err(error)?;
        all(rows)
    }

    async fn event_denials(
        &self,
        event: EventId,
        page: PageRequest<Keyset>,
    ) -> StoreResult<Vec<Denial>> {
        let after_at = page.after.map(|after| micros(after.at));
        let rows = sqlx::query_as::<_, DenialRow>(sqlx::AssertSqlSafe(format!(
            "{DENIAL_COLUMNS} WHERE event_id = ?
               AND (? IS NULL OR created_at < ? OR (created_at = ? AND id > ?))
             ORDER BY created_at DESC, id LIMIT ?"
        )))
        .bind(event.as_uuid())
        .bind(after_at)
        .bind(after_at)
        .bind(after_at)
        .bind(page.after.map(|after| after.id))
        .bind(page.limit)
        .fetch_all(&self.pool)
        .await
        .map_err(error)?;
        all(rows)
    }

    async fn account_denied(
        &self,
        organization: OrganizationId,
        event: EventId,
        account: AccountId,
    ) -> StoreResult<bool> {
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM denials
                            WHERE subject_id = ? AND subject_kind = 'account'
                              AND organization_id = ?
                              AND (event_id IS NULL OR event_id = ?))",
        )
        .bind(account.as_uuid())
        .bind(organization.as_uuid())
        .bind(event.as_uuid())
        .fetch_one(&self.pool)
        .await
        .map_err(error)
    }

    async fn denied_tickets(
        &self,
        event: EventId,
        page: PageRequest<Uuid>,
    ) -> StoreResult<Vec<DeniedTicket>> {
        let rows = sqlx::query_as::<_, DeniedTicketRow>(
            "SELECT t.id AS ticket_id, t.ticket_type_id,
                    CASE WHEN t.status = 'revoked' THEN 'revoked' ELSE 'denied' END AS reason
             FROM tickets t JOIN events e ON e.id = t.event_id
             WHERE t.event_id = ? AND (? IS NULL OR t.id > ?)
               AND (t.status = 'revoked'
                    OR EXISTS (SELECT 1 FROM denials d
                               WHERE d.organization_id = e.organization_id
                                 AND (d.event_id IS NULL OR d.event_id = t.event_id)
                                 AND ((d.subject_kind = 'ticket' AND d.subject_id = t.id)
                                      OR (d.subject_kind = 'account'
                                          AND d.subject_id = t.account_id))))
             ORDER BY t.id LIMIT ?",
        )
        .bind(event.as_uuid())
        .bind(page.after)
        .bind(page.after)
        .bind(page.limit)
        .fetch_all(&self.pool)
        .await
        .map_err(error)?;
        all(rows)
    }
}
