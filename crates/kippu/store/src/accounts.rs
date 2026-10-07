use async_trait::async_trait;
use kippu_domain::account::{Account, Identity, Organization};
use kippu_domain::validation::{Email, ProviderName, Subject};
use kippu_domain::{AccountId, OrganizationId, SessionId, Timestamp};

use crate::{Insertion, PageRequest, StoreResult};

/// What [`AccountStore::unlink_identity`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unlink {
    /// The identity was removed.
    Unlinked,
    /// The account has no such identity.
    NotLinked,
    /// The identity is the account's only way to sign in, so it was kept.
    LastSignInMethod,
}

/// A signed-in device: the hash of its refresh token and when it lapses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    /// Identity of the session.
    pub id: SessionId,
    /// Who is signed in.
    pub account_id: AccountId,
    /// SHA-256 of the refresh token, hex-encoded. The token itself is never stored.
    pub refresh_token_hash: String,
    /// When the session was created.
    pub created_at: Timestamp,
    /// When the refresh token stops working.
    pub expires_at: Timestamp,
}

/// The replacement for a session whose refresh token is being rotated. The account carries
/// over from the old session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRenewal {
    /// Identity of the new session.
    pub id: SessionId,
    /// SHA-256 of the new refresh token, hex-encoded.
    pub refresh_token_hash: String,
    /// When the rotation happens. The old session must not have expired by then.
    pub created_at: Timestamp,
    /// When the new refresh token stops working.
    pub expires_at: Timestamp,
}

/// Accounts, credentials, sessions and organizations.
#[async_trait]
pub trait AccountStore {
    /// Creates an account, with a password unless `password_hash` is `None`. Fails with
    /// `Conflict("email")` if the email is taken; any number of accounts may have none.
    async fn insert_account(
        &self,
        account: &Account,
        password_hash: Option<&str>,
    ) -> StoreResult<()>;

    /// Creates an account signed in through `identity` (whose `account_id` is `account.id`),
    /// without a password.
    ///
    /// **Contract:** atomic. If `(provider, subject)` is already linked, nothing is created
    /// and the linked account is returned as `Existing`, so concurrent first sign-ins of one
    /// person yield one account. Fails with `Conflict("email")`, creating nothing, if the
    /// email is taken.
    async fn create_account_with_identity(
        &self,
        account: &Account,
        identity: &Identity,
    ) -> StoreResult<Insertion<AccountId>>;

    /// Looks an account up by id.
    async fn account(&self, id: AccountId) -> StoreResult<Option<Account>>;

    /// Looks an account and its password hash (if it has a password) up by email, for sign-in.
    async fn account_credentials(
        &self,
        email: &Email,
    ) -> StoreResult<Option<(Account, Option<String>)>>;

    /// The account's password hash, if the account exists and has a password.
    async fn password_hash(&self, id: AccountId) -> StoreResult<Option<String>>;

    /// Sets an account's password. Returns whether the account exists.
    async fn set_password_hash(&self, id: AccountId, password_hash: &str) -> StoreResult<bool>;

    /// Sets an account's email. Returns whether the account exists; fails with
    /// `Conflict("email")` if another account has it.
    async fn set_email(&self, id: AccountId, email: &Email) -> StoreResult<bool>;

    /// Links an identity to an existing account. If `(provider, subject)` is already linked —
    /// to this account or another — returns that account as `Existing` and changes nothing.
    async fn insert_identity(&self, identity: &Identity) -> StoreResult<Insertion<AccountId>>;

    /// The account linked to `(provider, subject)`, if any.
    async fn identity_account(
        &self,
        provider: &ProviderName,
        subject: &Subject,
    ) -> StoreResult<Option<AccountId>>;

    /// An account's identities, oldest first.
    async fn identities(&self, account: AccountId) -> StoreResult<Vec<Identity>>;

    /// Removes one of an account's identities.
    ///
    /// **Contract:** an account always keeps a way to sign in. The identity is kept, and
    /// `LastSignInMethod` returned, when the account has no password and no other identity;
    /// concurrent unlinks of the same account must not both succeed in removing the last two.
    async fn unlink_identity(
        &self,
        account: AccountId,
        provider: &ProviderName,
        subject: &Subject,
    ) -> StoreResult<Unlink>;

    /// Lists accounts in id order.
    async fn list_accounts(&self, page: PageRequest) -> StoreResult<Vec<Account>>;

    /// Deletes an account and its sessions. Returns whether it existed.
    async fn delete_account(&self, id: AccountId) -> StoreResult<bool>;

    /// Stores a new session.
    async fn insert_session(&self, session: &Session) -> StoreResult<()>;

    /// Atomically replaces the unexpired session holding `refresh_token_hash` with `next`,
    /// returning the session's account.
    ///
    /// **Contract:** a refresh token can be rotated at most once; a second attempt (a replay)
    /// returns `None`.
    async fn rotate_session(
        &self,
        refresh_token_hash: &str,
        next: &SessionRenewal,
    ) -> StoreResult<Option<AccountId>>;

    /// Ends the session holding `refresh_token_hash`, if any.
    async fn revoke_session(&self, refresh_token_hash: &str) -> StoreResult<()>;

    /// Creates an organization. Fails with `Conflict("slug")` if the slug is taken.
    async fn insert_organization(&self, organization: &Organization) -> StoreResult<()>;

    /// Looks an organization up by id.
    async fn organization(&self, id: OrganizationId) -> StoreResult<Option<Organization>>;

    /// Lists organizations in id order.
    async fn list_organizations(&self, page: PageRequest) -> StoreResult<Vec<Organization>>;

    /// Makes an account a member of an organization. Idempotent.
    async fn add_member(&self, organization: OrganizationId, account: AccountId)
    -> StoreResult<()>;

    /// Removes a membership. Returns whether it existed.
    async fn remove_member(
        &self,
        organization: OrganizationId,
        account: AccountId,
    ) -> StoreResult<bool>;

    /// The organizations an account belongs to.
    async fn memberships(&self, account: AccountId) -> StoreResult<Vec<OrganizationId>>;
}
