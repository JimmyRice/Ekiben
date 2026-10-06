use kippu_domain::account::{Account, Role};
use kippu_domain::{OrganizationId, Timestamp};
use kippu_store::AuditEntry;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::service::{NewAccount, NewOrganization, Whoami};
pub use crate::auth::sessions::SessionResponse;

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

/// Set your email address.
#[derive(Debug, Deserialize, ToSchema)]
pub struct SetEmailRequest {
    /// The new sign-in address.
    pub email: String,
}

/// Set or change your password.
#[derive(Debug, Deserialize, ToSchema)]
pub struct SetPasswordRequest {
    /// Your current password. Leave it out only if the account has none yet (it was
    /// created through an external sign-in).
    pub current_password: Option<String>,
    /// 10 to 256 characters.
    pub new_password: String,
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

impl From<RegisterRequest> for NewAccount {
    fn from(request: RegisterRequest) -> Self {
        Self {
            email: request.email,
            password: request.password,
            display_name: request.display_name,
        }
    }
}

impl CreateAccountRequest {
    /// The account to create, and its role.
    pub(crate) fn into_parts(self) -> (NewAccount, Role) {
        let account = NewAccount {
            email: self.email,
            password: self.password,
            display_name: self.display_name,
        };
        (account, self.role)
    }
}

impl From<CreateOrganizationRequest> for NewOrganization {
    fn from(request: CreateOrganizationRequest) -> Self {
        Self {
            slug: request.slug,
            name: request.name,
        }
    }
}

impl From<Whoami> for Me {
    fn from(whoami: Whoami) -> Self {
        match whoami {
            Whoami::Root { key_name } => Self::Root { key_name },
            Whoami::Account {
                account,
                organizations,
            } => Self::Account {
                account,
                organizations,
            },
        }
    }
}

impl From<AuditEntry> for AuditEntryResponse {
    fn from(entry: AuditEntry) -> Self {
        Self {
            at: entry.at,
            actor: entry.actor,
            action: entry.action,
            target: entry.target,
        }
    }
}
