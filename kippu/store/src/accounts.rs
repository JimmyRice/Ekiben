use async_trait::async_trait;
use kippu_domain::account::{Account, Organization};
use kippu_domain::validation::Email;
use kippu_domain::{AccountId, OrganizationId, SessionId, Timestamp};

use crate::{PageRequest, StoreResult};

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
    /// Creates an account. Fails with `Conflict("email")` if the email is taken.
    async fn insert_account(&self, account: &Account, password_hash: &str) -> StoreResult<()>;

    /// Looks an account up by id.
    async fn account(&self, id: AccountId) -> StoreResult<Option<Account>>;

    /// Looks an account and its password hash up by email, for sign-in.
    async fn account_credentials(&self, email: &Email) -> StoreResult<Option<(Account, String)>>;

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
