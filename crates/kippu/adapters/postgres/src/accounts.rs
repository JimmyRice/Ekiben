use async_trait::async_trait;
use kippu_domain::account::{Account, Identity, Organization};
use kippu_domain::validation::{Email, ProviderName, Subject};
use kippu_domain::{AccountId, OrganizationId};
use kippu_store::{
    AccountStore, Insertion, PageRequest, Session, SessionRenewal, StoreResult, Unlink,
};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::convert::{AccountRow, CredentialsRow, IdentityRow, OrganizationRow, all, at, optional};
use crate::{PostgresStore, error, unique};

const INSERT_ACCOUNT: &str =
    "INSERT INTO accounts (id, email, display_name, role, password_hash, created_at)
     VALUES ($1, $2, $3, $4, $5, $6)";

const INSERT_IDENTITY: &str =
    "INSERT INTO identities (provider, subject, account_id, created_at) VALUES ($1, $2, $3, $4)
     ON CONFLICT DO NOTHING";

async fn insert_account(
    connection: &mut PgConnection,
    account: &Account,
    password_hash: Option<&str>,
) -> StoreResult<()> {
    sqlx::query(INSERT_ACCOUNT)
        .bind(account.id.as_uuid())
        .bind(account.email.as_ref().map(Email::as_str))
        .bind(&account.display_name)
        .bind(account.role.as_str())
        .bind(password_hash)
        .bind(at(account.created_at))
        .execute(connection)
        .await
        .map_err(unique("email"))?;
    Ok(())
}

async fn identity_account(
    connection: &mut PgConnection,
    provider: &ProviderName,
    subject: &Subject,
) -> StoreResult<Option<AccountId>> {
    let account = sqlx::query_scalar::<_, Uuid>(
        "SELECT account_id FROM identities WHERE provider = $1 AND subject = $2",
    )
    .bind(provider.as_str())
    .bind(subject.as_str())
    .fetch_optional(connection)
    .await
    .map_err(error)?;
    Ok(account.map(AccountId::from))
}

/// Inserts `identity` unless its `(provider, subject)` is taken; returns whether it did.
async fn try_insert_identity(
    connection: &mut PgConnection,
    identity: &Identity,
) -> StoreResult<bool> {
    let result = sqlx::query(INSERT_IDENTITY)
        .bind(identity.provider.as_str())
        .bind(identity.subject.as_str())
        .bind(identity.account_id.as_uuid())
        .bind(at(identity.created_at))
        .execute(connection)
        .await
        .map_err(error)?;
    Ok(result.rows_affected() == 1)
}

#[async_trait]
impl AccountStore for PostgresStore {
    async fn insert_account(
        &self,
        account: &Account,
        password_hash: Option<&str>,
    ) -> StoreResult<()> {
        let mut connection = self.pool.acquire().await.map_err(error)?;
        insert_account(&mut connection, account, password_hash).await
    }

    async fn create_account_with_identity(
        &self,
        account: &Account,
        identity: &Identity,
    ) -> StoreResult<Insertion<AccountId>> {
        let mut tx = self.pool.begin().await.map_err(error)?;
        if let Some(existing) =
            identity_account(&mut tx, &identity.provider, &identity.subject).await?
        {
            return Ok(Insertion::Existing(existing));
        }
        insert_account(&mut tx, account, None).await?;
        if !try_insert_identity(&mut tx, identity).await? {
            // A concurrent first sign-in won; dropping the transaction undoes our account.
            drop(tx);
            let mut connection = self.pool.acquire().await.map_err(error)?;
            let winner = identity_account(&mut connection, &identity.provider, &identity.subject)
                .await?
                .ok_or_else(|| kippu_store::StoreError::backend("identity vanished"))?;
            return Ok(Insertion::Existing(winner));
        }
        tx.commit().await.map_err(error)?;
        Ok(Insertion::Inserted)
    }

    async fn account(&self, id: AccountId) -> StoreResult<Option<Account>> {
        let row = sqlx::query_as::<_, AccountRow>(
            "SELECT id, email, display_name, role, created_at FROM accounts WHERE id = $1",
        )
        .bind(id.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(error)?;
        optional(row)
    }

    async fn account_credentials(
        &self,
        email: &Email,
    ) -> StoreResult<Option<(Account, Option<String>)>> {
        let row = sqlx::query_as::<_, CredentialsRow>(
            "SELECT id, email, display_name, role, created_at, password_hash
             FROM accounts WHERE email = $1",
        )
        .bind(email.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(error)?;
        row.map(|row| Ok((Account::try_from(row.account)?, row.password_hash)))
            .transpose()
    }

    async fn password_hash(&self, id: AccountId) -> StoreResult<Option<String>> {
        let hash = sqlx::query_scalar::<_, Option<String>>(
            "SELECT password_hash FROM accounts WHERE id = $1",
        )
        .bind(id.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(error)?;
        Ok(hash.flatten())
    }

    async fn set_password_hash(&self, id: AccountId, password_hash: &str) -> StoreResult<bool> {
        let result = sqlx::query("UPDATE accounts SET password_hash = $2 WHERE id = $1")
            .bind(id.as_uuid())
            .bind(password_hash)
            .execute(&self.pool)
            .await
            .map_err(error)?;
        Ok(result.rows_affected() == 1)
    }

    async fn set_email(&self, id: AccountId, email: &Email) -> StoreResult<bool> {
        let result = sqlx::query("UPDATE accounts SET email = $2 WHERE id = $1")
            .bind(id.as_uuid())
            .bind(email.as_str())
            .execute(&self.pool)
            .await
            .map_err(unique("email"))?;
        Ok(result.rows_affected() == 1)
    }

    async fn insert_identity(&self, identity: &Identity) -> StoreResult<Insertion<AccountId>> {
        let mut tx = self.pool.begin().await.map_err(error)?;
        if try_insert_identity(&mut tx, identity).await? {
            tx.commit().await.map_err(error)?;
            return Ok(Insertion::Inserted);
        }
        let existing = identity_account(&mut tx, &identity.provider, &identity.subject)
            .await?
            .ok_or_else(|| kippu_store::StoreError::backend("identity vanished"))?;
        Ok(Insertion::Existing(existing))
    }

    async fn identity_account(
        &self,
        provider: &ProviderName,
        subject: &Subject,
    ) -> StoreResult<Option<AccountId>> {
        let mut connection = self.pool.acquire().await.map_err(error)?;
        identity_account(&mut connection, provider, subject).await
    }

    async fn identities(&self, account: AccountId) -> StoreResult<Vec<Identity>> {
        let rows = sqlx::query_as::<_, IdentityRow>(
            "SELECT provider, subject, account_id, created_at FROM identities
             WHERE account_id = $1 ORDER BY created_at, provider, subject",
        )
        .bind(account.as_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(error)?;
        all(rows)
    }

    async fn unlink_identity(
        &self,
        account: AccountId,
        provider: &ProviderName,
        subject: &Subject,
    ) -> StoreResult<Unlink> {
        // Locking the account serializes unlinks of the same account: two of them cannot both
        // count two identities and remove one each.
        let mut tx = self.pool.begin().await.map_err(error)?;
        let Some(has_password) = sqlx::query_scalar::<_, bool>(
            "SELECT password_hash IS NOT NULL FROM accounts WHERE id = $1 FOR UPDATE",
        )
        .bind(account.as_uuid())
        .fetch_optional(&mut *tx)
        .await
        .map_err(error)?
        else {
            return Ok(Unlink::NotLinked);
        };
        if identity_account(&mut tx, provider, subject).await? != Some(account) {
            return Ok(Unlink::NotLinked);
        }
        let identities =
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM identities WHERE account_id = $1")
                .bind(account.as_uuid())
                .fetch_one(&mut *tx)
                .await
                .map_err(error)?;
        if !has_password && identities <= 1 {
            return Ok(Unlink::LastSignInMethod);
        }
        sqlx::query("DELETE FROM identities WHERE provider = $1 AND subject = $2")
            .bind(provider.as_str())
            .bind(subject.as_str())
            .execute(&mut *tx)
            .await
            .map_err(error)?;
        tx.commit().await.map_err(error)?;
        Ok(Unlink::Unlinked)
    }

    async fn list_accounts(&self, page: PageRequest) -> StoreResult<Vec<Account>> {
        let rows = sqlx::query_as::<_, AccountRow>(
            "SELECT id, email, display_name, role, created_at FROM accounts
             WHERE $1 IS NULL OR id > $1 ORDER BY id LIMIT $2",
        )
        .bind(page.after)
        .bind(i64::from(page.limit))
        .fetch_all(&self.pool)
        .await
        .map_err(error)?;
        all(rows)
    }

    async fn delete_account(&self, id: AccountId) -> StoreResult<bool> {
        let result = sqlx::query("DELETE FROM accounts WHERE id = $1")
            .bind(id.as_uuid())
            .execute(&self.pool)
            .await
            .map_err(error)?;
        Ok(result.rows_affected() == 1)
    }

    async fn insert_session(&self, session: &Session) -> StoreResult<()> {
        sqlx::query(
            "INSERT INTO sessions (id, account_id, refresh_token_hash, created_at, expires_at)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(session.id.as_uuid())
        .bind(session.account_id.as_uuid())
        .bind(&session.refresh_token_hash)
        .bind(at(session.created_at))
        .bind(at(session.expires_at))
        .execute(&self.pool)
        .await
        .map_err(error)?;
        Ok(())
    }

    async fn rotate_session(
        &self,
        refresh_token_hash: &str,
        next: &SessionRenewal,
    ) -> StoreResult<Option<AccountId>> {
        let mut tx = self.pool.begin().await.map_err(error)?;
        let account = sqlx::query_scalar::<_, Uuid>(
            "DELETE FROM sessions WHERE refresh_token_hash = $1 AND expires_at > $2
             RETURNING account_id",
        )
        .bind(refresh_token_hash)
        .bind(at(next.created_at))
        .fetch_optional(&mut *tx)
        .await
        .map_err(error)?;
        let Some(account) = account else {
            return Ok(None);
        };
        sqlx::query(
            "INSERT INTO sessions (id, account_id, refresh_token_hash, created_at, expires_at)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(next.id.as_uuid())
        .bind(account)
        .bind(&next.refresh_token_hash)
        .bind(at(next.created_at))
        .bind(at(next.expires_at))
        .execute(&mut *tx)
        .await
        .map_err(error)?;
        tx.commit().await.map_err(error)?;
        Ok(Some(account.into()))
    }

    async fn revoke_session(&self, refresh_token_hash: &str) -> StoreResult<()> {
        sqlx::query("DELETE FROM sessions WHERE refresh_token_hash = $1")
            .bind(refresh_token_hash)
            .execute(&self.pool)
            .await
            .map_err(error)?;
        Ok(())
    }

    async fn insert_organization(&self, organization: &Organization) -> StoreResult<()> {
        sqlx::query(
            "INSERT INTO organizations (id, slug, name, created_at) VALUES ($1, $2, $3, $4)",
        )
        .bind(organization.id.as_uuid())
        .bind(organization.slug.as_str())
        .bind(&organization.name)
        .bind(at(organization.created_at))
        .execute(&self.pool)
        .await
        .map_err(unique("slug"))?;
        Ok(())
    }

    async fn organization(&self, id: OrganizationId) -> StoreResult<Option<Organization>> {
        let row = sqlx::query_as::<_, OrganizationRow>(
            "SELECT id, slug, name, created_at FROM organizations WHERE id = $1",
        )
        .bind(id.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(error)?;
        optional(row)
    }

    async fn list_organizations(&self, page: PageRequest) -> StoreResult<Vec<Organization>> {
        let rows = sqlx::query_as::<_, OrganizationRow>(
            "SELECT id, slug, name, created_at FROM organizations
             WHERE $1 IS NULL OR id > $1 ORDER BY id LIMIT $2",
        )
        .bind(page.after)
        .bind(i64::from(page.limit))
        .fetch_all(&self.pool)
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
            "INSERT INTO memberships (organization_id, account_id) VALUES ($1, $2)
             ON CONFLICT DO NOTHING",
        )
        .bind(organization.as_uuid())
        .bind(account.as_uuid())
        .execute(&self.pool)
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
            sqlx::query("DELETE FROM memberships WHERE organization_id = $1 AND account_id = $2")
                .bind(organization.as_uuid())
                .bind(account.as_uuid())
                .execute(&self.pool)
                .await
                .map_err(error)?;
        Ok(result.rows_affected() == 1)
    }

    async fn memberships(&self, account: AccountId) -> StoreResult<Vec<OrganizationId>> {
        let ids = sqlx::query_scalar::<_, Uuid>(
            "SELECT organization_id FROM memberships WHERE account_id = $1 ORDER BY organization_id",
        )
        .bind(account.as_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(error)?;
        Ok(ids.into_iter().map(Into::into).collect())
    }
}
