//! Who is calling.

use kippu_domain::account::Role;
use kippu_domain::{AccountId, OrganizationId};

use crate::error::ApiError;

/// Who is making a request.
///
/// Use it as a handler argument to require authentication, or as `Option<Principal>` for
/// endpoints anonymous visitors may call too.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Principal {
    /// The deployment owner, proven with a root key from the configuration.
    Root {
        /// Name of the root key that signed the token.
        key_name: String,
    },
    /// A signed-in account.
    Account {
        /// The account.
        id: AccountId,
        /// Its role when the access token was issued.
        role: Role,
        /// Its organizations when the access token was issued.
        organizations: Vec<OrganizationId>,
    },
}

impl Principal {
    /// The principal's role.
    pub fn role(&self) -> Role {
        match self {
            Self::Root { .. } => Role::Root,
            Self::Account { role, .. } => *role,
        }
    }

    /// The account, unless this is root.
    pub fn account_id(&self) -> Option<AccountId> {
        match self {
            Self::Root { .. } => None,
            Self::Account { id, .. } => Some(*id),
        }
    }

    /// The account, or 403 for root, which has no account to act as (e.g. to buy tickets).
    pub fn require_account(&self) -> Result<AccountId, ApiError> {
        self.account_id().ok_or_else(ApiError::forbidden)
    }

    /// How the principal appears in the audit log: `root:<key>` or `account:<id>`.
    pub fn actor(&self) -> String {
        match self {
            Self::Root { key_name } => format!("root:{key_name}"),
            Self::Account { id, .. } => format!("account:{id}"),
        }
    }
}
