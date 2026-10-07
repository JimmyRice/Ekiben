use async_trait::async_trait;
use kippu_domain::denial::{Denial, DeniedTicket};
use kippu_domain::{AccountId, DenialId, EventId, OrganizationId};
use kippu_store::{DenialStore, Insertion, Keyset, PageRequest, StoreError, StoreResult};
use uuid::Uuid;

use crate::convert::{DenialRow, DeniedTicketRow, all, at, optional};
use crate::{PostgresStore, error};

const DENIAL_COLUMNS: &str = "SELECT id, organization_id, event_id, subject_kind, subject_id, \
     note, created_at, updated_at, version FROM denials";

#[async_trait]
impl DenialStore for PostgresStore {
    async fn insert_denial(&self, denial: &Denial) -> StoreResult<Insertion<Denial>> {
        let inserted = sqlx::query(
            "INSERT INTO denials (id, organization_id, event_id, subject_kind, subject_id, note,
                                  created_at, updated_at, version)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
             ON CONFLICT (id) DO NOTHING",
        )
        .bind(denial.id.as_uuid())
        .bind(denial.organization_id.as_uuid())
        .bind(denial.event_id.map(|event| event.as_uuid()))
        .bind(denial.subject.kind())
        .bind(denial.subject.id())
        .bind(&denial.note)
        .bind(at(denial.created_at))
        .bind(at(denial.updated_at))
        .bind(denial.version)
        .execute(&self.pool)
        .await
        .map_err(error)?;
        if inserted.rows_affected() == 1 {
            return Ok(Insertion::Inserted);
        }
        let existing = sqlx::query_as::<_, DenialRow>(sqlx::AssertSqlSafe(format!(
            "{DENIAL_COLUMNS} WHERE id = $1"
        )))
        .bind(denial.id.as_uuid())
        .fetch_one(&self.pool)
        .await
        .map_err(error)?;
        Ok(Insertion::Existing(existing.try_into()?))
    }

    async fn denial(&self, id: DenialId) -> StoreResult<Option<Denial>> {
        let row = sqlx::query_as::<_, DenialRow>(sqlx::AssertSqlSafe(format!(
            "{DENIAL_COLUMNS} WHERE id = $1"
        )))
        .bind(id.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(error)?;
        optional(row)
    }

    async fn update_denial(&self, denial: &Denial, expected_version: i64) -> StoreResult<()> {
        let updated = sqlx::query(
            "UPDATE denials SET note = $2, updated_at = $3, version = $4
             WHERE id = $1 AND version = $5",
        )
        .bind(denial.id.as_uuid())
        .bind(&denial.note)
        .bind(at(denial.updated_at))
        .bind(denial.version)
        .bind(expected_version)
        .execute(&self.pool)
        .await
        .map_err(error)?;
        if updated.rows_affected() == 1 {
            Ok(())
        } else {
            Err(StoreError::Conflict("version"))
        }
    }

    async fn delete_denial(&self, id: DenialId) -> StoreResult<bool> {
        let deleted = sqlx::query("DELETE FROM denials WHERE id = $1")
            .bind(id.as_uuid())
            .execute(&self.pool)
            .await
            .map_err(error)?;
        Ok(deleted.rows_affected() == 1)
    }

    async fn organization_denials(
        &self,
        organization: OrganizationId,
        page: PageRequest<Keyset>,
    ) -> StoreResult<Vec<Denial>> {
        let rows = sqlx::query_as::<_, DenialRow>(sqlx::AssertSqlSafe(format!(
            "{DENIAL_COLUMNS} WHERE organization_id = $1 AND event_id IS NULL
               AND ($2 IS NULL OR created_at < $2 OR (created_at = $2 AND id > $3))
             ORDER BY created_at DESC, id LIMIT $4"
        )))
        .bind(organization.as_uuid())
        .bind(page.after.map(|after| at(after.at)))
        .bind(page.after.map(|after| after.id))
        .bind(i64::from(page.limit))
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
        let rows = sqlx::query_as::<_, DenialRow>(sqlx::AssertSqlSafe(format!(
            "{DENIAL_COLUMNS} WHERE event_id = $1
               AND ($2 IS NULL OR created_at < $2 OR (created_at = $2 AND id > $3))
             ORDER BY created_at DESC, id LIMIT $4"
        )))
        .bind(event.as_uuid())
        .bind(page.after.map(|after| at(after.at)))
        .bind(page.after.map(|after| after.id))
        .bind(i64::from(page.limit))
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
                            WHERE subject_id = $3 AND subject_kind = 'account'
                              AND organization_id = $1
                              AND (event_id IS NULL OR event_id = $2))",
        )
        .bind(organization.as_uuid())
        .bind(event.as_uuid())
        .bind(account.as_uuid())
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
             WHERE t.event_id = $1 AND ($2 IS NULL OR t.id > $2)
               AND (t.status = 'revoked'
                    OR EXISTS (SELECT 1 FROM denials d
                               WHERE d.organization_id = e.organization_id
                                 AND (d.event_id IS NULL OR d.event_id = t.event_id)
                                 AND ((d.subject_kind = 'ticket' AND d.subject_id = t.id)
                                      OR (d.subject_kind = 'account'
                                          AND d.subject_id = t.account_id))))
             ORDER BY t.id LIMIT $3",
        )
        .bind(event.as_uuid())
        .bind(page.after)
        .bind(i64::from(page.limit))
        .fetch_all(&self.pool)
        .await
        .map_err(error)?;
        all(rows)
    }
}
