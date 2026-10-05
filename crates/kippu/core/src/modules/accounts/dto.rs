use kippu_domain::account::{Account, Role};
use kippu_domain::{OrganizationId, Timestamp};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::auth::tokens::IssuedToken;

/// Sign up as a user.
#[derive(Debug, Deserialize, ToSchema)]
pub struct RegisterRequest {
    /// Sign-in address.
    pub email: String,
    /// 10 to 256 characters.
    pub password: String,
    /// Name shown to others.
    pub display_name: String,
}

/// Sign in with email and password.
#[derive(Debug, Deserialize, ToSchema)]
pub struct LoginRequest {
    /// Sign-in address.
    pub email: String,
    /// Password.
    pub password: String,
}

/// Exchange a refresh token for new tokens. The old refresh token stops working.
#[derive(Debug, Deserialize, ToSchema)]
pub struct RefreshRequest {
    /// The refresh token from the previous sign-in or refresh.
    pub refresh_token: String,
}

/// A signed-in session.
#[derive(Debug, Serialize, ToSchema)]
pub struct SessionResponse {
    /// Short-lived bearer token for API calls.
    pub access_token: IssuedToken,
    /// Long-lived, single-use token to obtain the next access token.
    pub refresh_token: String,
    /// When the refresh token lapses.
    pub refresh_expires_at: Timestamp,
    /// The signed-in account.
    pub account: Account,
}

/// Who the caller is.
#[derive(Debug, Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Me {
    /// Root, signed in with a key.
    Root {
        /// Name of the root key used.
        key_name: String,
    },
    /// An account.
    Account {
        /// The account.
        account: Account,
        /// Organizations it belongs to.
        organizations: Vec<OrganizationId>,
    },
}

/// Create an account of a role below the caller's.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateAccountRequest {
    /// Sign-in address.
    pub email: String,
    /// Initial password, 10 to 256 characters.
    pub password: String,
    /// Name shown to others.
    pub display_name: String,
    /// `user`, `organizer` or `admin`.
    pub role: Role,
}

/// Create an organization.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateOrganizationRequest {
    /// URL-friendly unique name.
    pub slug: String,
    /// Display name.
    pub name: String,
}

/// One audit log entry.
#[derive(Debug, Serialize, ToSchema)]
pub struct AuditEntryResponse {
    /// When it happened.
    pub at: Timestamp,
    /// Who did it.
    pub actor: String,
    /// What they did.
    pub action: String,
    /// What they did it to.
    pub target: String,
}
