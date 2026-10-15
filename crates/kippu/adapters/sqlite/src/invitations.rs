use async_trait::async_trait;
use kippu_domain::invitation::Invitation;
use kippu_domain::{AccountId, SaleId};
use kippu_store::{Insertion, InvitationStore, Keyset, PageRequest, StoreResult};

use crate::convert::{InvitationRow, all, micros};
use crate::{SqliteStore, error};

const INVITATION_COLUMNS: &str = "SELECT sale_id, account_id, created_at FROM sale_invitations";

#[async_trait]
impl InvitationStore for SqliteStore {
    async fn insert_invitation(
        &self,
        invitation: &Invitation,
    ) -> StoreResult<Insertion<Invitation>> {
        let inserted = sqlx::query(
            "INSERT INTO sale_invitations (sale_id, account_id, created_at) VALUES (?1, ?2, ?3)
             ON CONFLICT (sale_id, account_id) DO NOTHING",
        )
        .bind(invitation.sale_id.as_uuid())
        .bind(invitation.account_id.as_uuid())
        .bind(micros(invitation.created_at))
        .execute(&self.writer)
        .await
        .map_err(error)?;
        if inserted.rows_affected() == 1 {
            return Ok(Insertion::Inserted);
        }
        let existing = sqlx::query_as::<_, InvitationRow>(sqlx::AssertSqlSafe(format!(
            "{INVITATION_COLUMNS} WHERE sale_id = ?1 AND account_id = ?2"
        )))
        .bind(invitation.sale_id.as_uuid())
        .bind(invitation.account_id.as_uuid())
        .fetch_one(&self.writer)
        .await
        .map_err(error)?;
        Ok(Insertion::Existing(existing.try_into()?))
    }

    async fn delete_invitation(&self, sale: SaleId, account: AccountId) -> StoreResult<bool> {
        let deleted =
            sqlx::query("DELETE FROM sale_invitations WHERE sale_id = ?1 AND account_id = ?2")
                .bind(sale.as_uuid())
                .bind(account.as_uuid())
                .execute(&self.writer)
                .await
                .map_err(error)?;
        Ok(deleted.rows_affected() == 1)
    }

    async fn is_invited(&self, sale: SaleId, account: AccountId) -> StoreResult<bool> {
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM sale_invitations WHERE sale_id = ?1 AND account_id = ?2)",
        )
        .bind(sale.as_uuid())
        .bind(account.as_uuid())
        .fetch_one(&self.reader)
        .await
        .map_err(error)
    }

    async fn invitations(
        &self,
        sale: SaleId,
        page: PageRequest<Keyset>,
    ) -> StoreResult<Vec<Invitation>> {
        let rows = sqlx::query_as::<_, InvitationRow>(sqlx::AssertSqlSafe(format!(
            "{INVITATION_COLUMNS} WHERE sale_id = ?1
               AND (?2 IS NULL OR created_at < ?2 OR (created_at = ?2 AND account_id > ?3))
             ORDER BY created_at DESC, account_id LIMIT ?4"
        )))
        .bind(sale.as_uuid())
        .bind(page.after.map(|after| micros(after.at)))
        .bind(page.after.map(|after| after.id))
        .bind(page.limit)
        .fetch_all(&self.reader)
        .await
        .map_err(error)?;
        all(rows)
    }
}
