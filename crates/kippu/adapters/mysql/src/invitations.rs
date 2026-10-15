use async_trait::async_trait;
use kippu_domain::invitation::Invitation;
use kippu_domain::{AccountId, SaleId};
use kippu_store::{Insertion, InvitationStore, Keyset, PageRequest, StoreResult};

use crate::convert::{InvitationRow, all, micros};
use crate::{MySqlStore, error, is_duplicate};

const INVITATION_COLUMNS: &str = "SELECT sale_id, account_id, created_at FROM sale_invitations";

#[async_trait]
impl InvitationStore for MySqlStore {
    async fn insert_invitation(
        &self,
        invitation: &Invitation,
    ) -> StoreResult<Insertion<Invitation>> {
        let inserted = sqlx::query(
            "INSERT INTO sale_invitations (sale_id, account_id, created_at) VALUES (?, ?, ?)",
        )
        .bind(invitation.sale_id.as_uuid())
        .bind(invitation.account_id.as_uuid())
        .bind(micros(invitation.created_at))
        .execute(&self.pool)
        .await;
        match inserted {
            Ok(_) => return Ok(Insertion::Inserted),
            Err(failure) if is_duplicate(&failure) => {}
            Err(failure) => return Err(error(failure)),
        }
        let existing = sqlx::query_as::<_, InvitationRow>(sqlx::AssertSqlSafe(format!(
            "{INVITATION_COLUMNS} WHERE sale_id = ? AND account_id = ?"
        )))
        .bind(invitation.sale_id.as_uuid())
        .bind(invitation.account_id.as_uuid())
        .fetch_one(&self.pool)
        .await
        .map_err(error)?;
        Ok(Insertion::Existing(existing.try_into()?))
    }

    async fn delete_invitation(&self, sale: SaleId, account: AccountId) -> StoreResult<bool> {
        let deleted =
            sqlx::query("DELETE FROM sale_invitations WHERE sale_id = ? AND account_id = ?")
                .bind(sale.as_uuid())
                .bind(account.as_uuid())
                .execute(&self.pool)
                .await
                .map_err(error)?;
        Ok(deleted.rows_affected() == 1)
    }

    async fn is_invited(&self, sale: SaleId, account: AccountId) -> StoreResult<bool> {
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM sale_invitations WHERE sale_id = ? AND account_id = ?)",
        )
        .bind(sale.as_uuid())
        .bind(account.as_uuid())
        .fetch_one(&self.pool)
        .await
        .map_err(error)
    }

    async fn invitations(
        &self,
        sale: SaleId,
        page: PageRequest<Keyset>,
    ) -> StoreResult<Vec<Invitation>> {
        let after_at = page.after.map(|after| micros(after.at));
        let rows = sqlx::query_as::<_, InvitationRow>(sqlx::AssertSqlSafe(format!(
            "{INVITATION_COLUMNS} WHERE sale_id = ?
               AND (? IS NULL OR created_at < ? OR (created_at = ? AND account_id > ?))
             ORDER BY created_at DESC, account_id LIMIT ?"
        )))
        .bind(sale.as_uuid())
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
}
