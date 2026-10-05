use async_trait::async_trait;
use kippu_domain::account::{Account, Organization};
use kippu_domain::validation::Email;
use kippu_domain::{AccountId, OrganizationId};
use kippu_store::{AccountStore, PageRequest, Session, SessionRenewal, StoreResult};
use uuid::Uuid;

use crate::convert::{AccountRow, CredentialsRow, OrganizationRow, all, micros, optional};
use crate::{SqliteStore, error, unique};

#[async_trait]
impl AccountStore for SqliteStore {
    async fn insert_account(&self, account: &Account, password_hash: &str) -> StoreResult<()> {
        sqlx::query(
            "INSERT INTO accounts (id, email, display_name, role, password_hash, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )
        .bind(account.id.as_uuid())
        .bind(account.email.as_str())
        .bind(&account.display_name)
        .bind(account.role.as_str())
        .bind(password_hash)
        .bind(micros(account.created_at))
        .execute(&self.writer)
        .await
        .map_err(unique("email"))?;
        Ok(())
    }

    async fn account(&self, id: AccountId) -> StoreResult<Option<Account>> {
        let row = sqlx::query_as::<_, AccountRow>(
            "SELECT id, email, display_name, role, created_at FROM accounts WHERE id = ?1",
        )
        .bind(id.as_uuid())
        .fetch_optional(&self.reader)
        .await
        .map_err(error)?;
        optional(row)
    }

    async fn account_credentials(&self, email: &Email) -> StoreResult<Option<(Account, String)>> {
        let row = sqlx::query_as::<_, CredentialsRow>(
            "SELECT id, email, display_name, role, created_at, password_hash
             FROM accounts WHERE email = ?1",
        )
        .bind(email.as_str())
        .fetch_optional(&self.reader)
        .await
        .map_err(error)?;
        row.map(|row| Ok((Account::try_from(row.account)?, row.password_hash)))
            .transpose()
    }

    async fn list_accounts(&self, page: PageRequest) -> StoreResult<Vec<Account>> {
        let rows = sqlx::query_as::<_, AccountRow>(
            "SELECT id, email, display_name, role, created_at FROM accounts
             WHERE ?1 IS NULL OR id > ?1 ORDER BY id LIMIT ?2",
        )
        .bind(page.after)
        .bind(page.limit)
        .fetch_all(&self.reader)
        .await
        .map_err(error)?;
        all(rows)
    }

    async fn delete_account(&self, id: AccountId) -> StoreResult<bool> {
        let result = sqlx::query("DELETE FROM accounts WHERE id = ?1")
            .bind(id.as_uuid())
            .execute(&self.writer)
            .await
            .map_err(error)?;
        Ok(result.rows_affected() == 1)
    }

    async fn insert_session(&self, session: &Session) -> StoreResult<()> {
        sqlx::query(
            "INSERT INTO sessions (id, account_id, refresh_token_hash, created_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .bind(session.id.as_uuid())
        .bind(session.account_id.as_uuid())
        .bind(&session.refresh_token_hash)
        .bind(micros(session.created_at))
        .bind(micros(session.expires_at))
        .execute(&self.writer)
        .await
        .map_err(error)?;
        Ok(())
    }

    async fn rotate_session(
        &self,
        refresh_token_hash: &str,
        next: &SessionRenewal,
    ) -> StoreResult<Option<AccountId>> {
        let mut tx = self
            .writer
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(error)?;
        let account = sqlx::query_scalar::<_, Uuid>(
            "DELETE FROM sessions WHERE refresh_token_hash = ?1 AND expires_at > ?2
             RETURNING account_id",
        )
        .bind(refresh_token_hash)
        .bind(micros(next.created_at))
        .fetch_optional(&mut *tx)
        .await
        .map_err(error)?;
        let Some(account) = account else {
            return Ok(None);
        };
        sqlx::query(
            "INSERT INTO sessions (id, account_id, refresh_token_hash, created_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .bind(next.id.as_uuid())
        .bind(account)
        .bind(&next.refresh_token_hash)
        .bind(micros(next.created_at))
        .bind(micros(next.expires_at))
        .execute(&mut *tx)
        .await
        .map_err(error)?;
        tx.commit().await.map_err(error)?;
        Ok(Some(account.into()))
    }

    async fn revoke_session(&self, refresh_token_hash: &str) -> StoreResult<()> {
        sqlx::query("DELETE FROM sessions WHERE refresh_token_hash = ?1")
            .bind(refresh_token_hash)
            .execute(&self.writer)
            .await
            .map_err(error)?;
        Ok(())
    }

    async fn insert_organization(&self, organization: &Organization) -> StoreResult<()> {
        sqlx::query(
            "INSERT INTO organizations (id, slug, name, created_at) VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(organization.id.as_uuid())
        .bind(organization.slug.as_str())
        .bind(&organization.name)
        .bind(micros(organization.created_at))
        .execute(&self.writer)
        .await
        .map_err(unique("slug"))?;
        Ok(())
    }

    async fn organization(&self, id: OrganizationId) -> StoreResult<Option<Organization>> {
        let row = sqlx::query_as::<_, OrganizationRow>(
            "SELECT id, slug, name, created_at FROM organizations WHERE id = ?1",
        )
        .bind(id.as_uuid())
        .fetch_optional(&self.reader)
        .await
        .map_err(error)?;
        optional(row)
    }

    async fn list_organizations(&self, page: PageRequest) -> StoreResult<Vec<Organization>> {
        let rows = sqlx::query_as::<_, OrganizationRow>(
            "SELECT id, slug, name, created_at FROM organizations
             WHERE ?1 IS NULL OR id > ?1 ORDER BY id LIMIT ?2",
        )
        .bind(page.after)
        .bind(page.limit)
        .fetch_all(&self.reader)
        .await
        .map_err(error)?;
        all(rows)
    }

    async fn add_member(
        &self,
        organization: OrganizationId,
        account: AccountId,
    ) -> StoreResult<()> {
        sqlx::query(
            "INSERT INTO memberships (organization_id, account_id) VALUES (?1, ?2)
             ON CONFLICT DO NOTHING",
        )
        .bind(organization.as_uuid())
        .bind(account.as_uuid())
        .execute(&self.writer)
        .await
        .map_err(error)?;
        Ok(())
    }

    async fn remove_member(
        &self,
        organization: OrganizationId,
        account: AccountId,
    ) -> StoreResult<bool> {
        let result =
            sqlx::query("DELETE FROM memberships WHERE organization_id = ?1 AND account_id = ?2")
                .bind(organization.as_uuid())
                .bind(account.as_uuid())
                .execute(&self.writer)
                .await
                .map_err(error)?;
        Ok(result.rows_affected() == 1)
    }

    async fn memberships(&self, account: AccountId) -> StoreResult<Vec<OrganizationId>> {
        let ids = sqlx::query_scalar::<_, Uuid>(
            "SELECT organization_id FROM memberships WHERE account_id = ?1 ORDER BY organization_id",
        )
        .bind(account.as_uuid())
        .fetch_all(&self.reader)
        .await
        .map_err(error)?;
        Ok(ids.into_iter().map(Into::into).collect())
    }
}
