//! Authentication (who is calling) and authorization (what they may do).

pub mod jwt;
pub mod password;
pub mod policy;
pub mod tokens;

use axum::extract::{FromRequestParts, OptionalFromRequestParts};
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use kippu_domain::account::Role;
use kippu_domain::{AccountId, OrganizationId};

use crate::app::AppState;
use crate::error::ApiError;

pub use policy::{Permission, Policy, Scope};

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

fn bearer(parts: &Parts) -> Result<Option<&str>, ApiError> {
    let Some(value) = parts.headers.get(AUTHORIZATION) else {
        return Ok(None);
    };
    value
        .to_str()
        .ok()
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(Some)
        .ok_or_else(|| ApiError::unauthenticated("expected `Authorization: Bearer <token>`"))
}

impl FromRequestParts<AppState> for Principal {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        <Self as OptionalFromRequestParts<AppState>>::from_request_parts(parts, state)
            .await?
            .ok_or_else(|| ApiError::unauthenticated("sign in first"))
    }
}

impl OptionalFromRequestParts<AppState> for Principal {
    type Rejection = ApiError;

    /// Verifying a token needs no I/O, so this resolves immediately.
    fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> impl Future<Output = Result<Option<Self>, ApiError>> + Send {
        let principal = bearer(parts).and_then(|token| {
            token
                .map(|token| state.tokens().authenticate(token, state.now()))
                .transpose()
        });
        if let Ok(Some(principal)) = &principal {
            crate::http::trace::record_caller(principal);
        }
        std::future::ready(principal)
    }
}
