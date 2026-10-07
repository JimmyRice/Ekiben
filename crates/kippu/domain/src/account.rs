//! Accounts, roles and organizations.

use serde::{Deserialize, Serialize};

use crate::validation::{Email, ProviderName, Slug, Subject};
use crate::{AccountId, OrganizationId, Timestamp};

/// What an authenticated principal may do. Ordered from least to most privileged.
///
/// Anonymous visitors have no role at all. `Root` is never stored: it is proven per request
/// with a key from the deployment's configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Signed up to favourite events and buy tickets.
    User,
    /// Manages the events, sales and ticket types of their organizations.
    Organizer,
    /// Manages everything except root, including organizations, organizers and attestors.
    Admin,
    /// The deployment owner. Can do anything.
    Root,
}

impl Role {
    /// Whether an account with this role may create, change or delete accounts of `other`.
    ///
    /// Only admins and root manage accounts, and only those of strictly lower roles.
    /// Organizers manage events, never people.
    pub fn can_manage(self, other: Self) -> bool {
        matches!(self, Self::Admin | Self::Root) && self > other
    }

    /// Whether accounts can be created with this role. Root lives only in configuration.
    pub const fn is_assignable(self) -> bool {
        !matches!(self, Self::Root)
    }

    /// The role's name as stored and serialized.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Organizer => "organizer",
            Self::Admin => "admin",
            Self::Root => "root",
        }
    }
}

impl std::str::FromStr for Role {
    type Err = crate::ValidationError;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        match name {
            "user" => Ok(Self::User),
            "organizer" => Ok(Self::Organizer),
            "admin" => Ok(Self::Admin),
            "root" => Ok(Self::Root),
            _ => Err(crate::ValidationError::new("role", "is not a known role")),
        }
    }
}

/// A person who can sign in. Credentials are stored separately: a password, external
/// [`Identity`]s (Sign in with Apple, WeChat, …), or both.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Account {
    /// Identity of the account.
    pub id: AccountId,
    /// Sign-in address, unique across accounts. Accounts created through an external sign-in
    /// that shared no address have none.
    pub email: Option<Email>,
    /// Name shown to other people.
    pub display_name: String,
    /// What the account may do.
    pub role: Role,
    /// When the account was created.
    pub created_at: Timestamp,
}

/// A sign-in method outside Kippu, linked to an account: the account is whoever the
/// provider vouches for as `subject`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Identity {
    /// Which provider, e.g. `apple` or `wechat`.
    pub provider: ProviderName,
    /// The provider's stable identifier for the person, e.g. Apple's `sub` or WeChat's
    /// `unionid`. Unique per provider.
    pub subject: Subject,
    /// The linked account.
    pub account_id: AccountId,
    /// When it was linked.
    pub created_at: Timestamp,
}

/// A convention organizer (主办方). Organizer accounts act only within their organizations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Organization {
    /// Identity of the organization.
    pub id: OrganizationId,
    /// URL-friendly unique name.
    pub slug: Slug,
    /// Display name.
    pub name: String,
    /// When the organization was created.
    pub created_at: Timestamp,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_strictly_lower_roles_can_be_managed() {
        assert!(Role::Root.can_manage(Role::Admin));
        assert!(Role::Admin.can_manage(Role::Organizer));
        assert!(Role::Admin.can_manage(Role::User));
        assert!(!Role::Admin.can_manage(Role::Admin));
        assert!(!Role::Organizer.can_manage(Role::User));
        assert!(!Role::User.can_manage(Role::User));
    }

    #[test]
    fn root_is_never_assignable() {
        assert!(!Role::Root.is_assignable());
        assert!(Role::Admin.is_assignable());
    }
}
