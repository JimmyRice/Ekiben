//! Signing in through someone else: Sign in with Apple, WeChat, QQ, an organization's own
//! OAuth server…
//!
//! Kippu ships no client for any provider. A [`Module`](crate::Module) of yours talks to the
//! provider — redirects, `state` and PKCE, the token exchange, checking the provider's
//! signature — and once it knows who the person is, it hands an [`ExternalIdentity`] to
//! [`link_or_create`] and signs the account in with [`sessions::issue`](super::sessions::issue).
//! The person then holds ordinary Kippu tokens. `docs/external-login.md` walks through it and
//! `kippu-server`'s `external_login` example is a complete module.
//!
//! ```ignore
//! let identity = ExternalIdentity::new("apple", claims.sub)
//!     .email(claims.email, claims.email_verified)
//!     .display_name(name);
//! let linked = external::link_or_create(&state, identity).await?;
//! Ok(Json(sessions::issue(&state, linked.account).await?))
//! ```

use kippu_domain::AccountId;
use kippu_domain::account::{Account, Identity, Role};
use kippu_domain::validation::{Email, ProviderName, Subject};
use kippu_store::Insertion;

use crate::app::AppState;
use crate::auth::Principal;
use crate::error::{ApiError, ApiResult, ProblemKind};

/// Display name for accounts whose provider shared none. The person can change it later.
const DEFAULT_DISPLAY_NAME: &str = "User";

/// Who a provider says someone is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalIdentity {
    /// Your name for the provider, e.g. `apple`: 1–32 characters of `a-z`, `0-9`, `-`, `_`.
    /// Keep it stable; it is half of the identity's key.
    pub provider: String,
    /// The provider's stable identifier for the person — Apple's `sub`, WeChat's `unionid`
    /// (or `openid` without an open platform account). Never an email or a display name,
    /// which can change hands.
    pub subject: String,
    /// The person's email, if the provider shared one.
    pub email: Option<String>,
    /// Whether the provider vouches that the person controls `email`.
    pub email_verified: bool,
    /// A name to show, if the provider shared one.
    pub display_name: Option<String>,
}

impl ExternalIdentity {
    /// An identity with only the provider and subject.
    pub fn new(provider: impl Into<String>, subject: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            subject: subject.into(),
            email: None,
            email_verified: false,
            display_name: None,
        }
    }

    /// Adds the email the provider shared, and whether it verified it.
    #[must_use]
    pub fn email(mut self, email: impl Into<String>, verified: bool) -> Self {
        self.email = Some(email.into());
        self.email_verified = verified;
        self
    }

    /// Adds the name the provider shared.
    #[must_use]
    pub fn display_name(mut self, display_name: impl Into<String>) -> Self {
        self.display_name = Some(display_name.into());
        self
    }

    fn key(&self) -> ApiResult<(ProviderName, Subject)> {
        Ok((
            ProviderName::new(self.provider.clone())?,
            Subject::new(self.subject.clone())?,
        ))
    }

    /// The email, if the provider verified it. Unverified addresses are not stored: anyone can
    /// type someone else's address into an account at a provider.
    fn verified_email(&self) -> ApiResult<Option<Email>> {
        Ok(match &self.email {
            Some(email) if self.email_verified => Some(Email::new(email.clone())?),
            _ => None,
        })
    }

    fn name(&self) -> String {
        self.display_name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map_or_else(
                || DEFAULT_DISPLAY_NAME.to_owned(),
                |name| name.chars().take(100).collect(),
            )
    }
}

/// The account an external sign-in resolved to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Linked {
    /// The account to sign in.
    pub account: Account,
    /// Whether it was created by this sign-in.
    pub created: bool,
}

/// Finds the account an external identity belongs to, linking or creating it as needed:
///
/// 1. An identity linked before signs in to its account.
/// 2. Otherwise, if `auth.link_by_verified_email` is on and the provider verified the email,
///    the identity is linked to the account with that email. Off by default: Kippu itself does
///    not verify the emails people register with, so whoever registered an address first
///    would gain the provider's sign-in for it.
/// 3. Otherwise a new `user` account is created, with the email if the provider verified it.
///    If another account already has that email, this fails with `409
///    email-already-registered`: the person should sign in to that account and link the
///    provider with [`link`].
///
/// Concurrent first sign-ins of the same person yield one account. Links are audited.
pub async fn link_or_create(state: &AppState, identity: ExternalIdentity) -> ApiResult<Linked> {
    let (provider, subject) = identity.key()?;
    let store = state.store();
    if let Some(account) = store.identity_account(&provider, &subject).await? {
        return existing(state, account).await;
    }

    let email = identity.verified_email()?;
    let now = state.now();
    if state.config().auth.link_by_verified_email
        && let Some(email) = &email
        && let Some((account, _)) = store.account_credentials(email).await?
    {
        let link = Identity {
            provider,
            subject,
            account_id: account.id,
            created_at: now,
        };
        return match store.insert_identity(&link).await? {
            Insertion::Inserted => {
                audit_link(state, &account, &link).await?;
                Ok(Linked {
                    account,
                    created: false,
                })
            }
            Insertion::Existing(winner) => existing(state, winner).await,
        };
    }

    let account = Account {
        id: AccountId::generate(),
        email,
        display_name: identity.name(),
        role: Role::User,
        created_at: now,
    };
    let link = Identity {
        provider,
        subject,
        account_id: account.id,
        created_at: now,
    };
    match store.create_account_with_identity(&account, &link).await {
        Ok(Insertion::Inserted) => {
            audit_link(state, &account, &link).await?;
            Ok(Linked {
                account,
                created: true,
            })
        }
        Ok(Insertion::Existing(winner)) => existing(state, winner).await,
        Err(kippu_store::StoreError::Conflict("email")) => Err(ApiError::new(
            axum::http::StatusCode::CONFLICT,
            ProblemKind::EMAIL_ALREADY_REGISTERED,
            "an account with this email exists; sign in to it and link this provider there",
        )),
        Err(error) => Err(error.into()),
    }
}

/// Links an external identity to a signed-in account, e.g. from its settings page.
/// Linking the same identity again is harmless; one linked to another account fails with
/// `409 identity-linked-elsewhere`.
pub async fn link(
    state: &AppState,
    account: &Account,
    identity: &ExternalIdentity,
) -> ApiResult<()> {
    let (provider, subject) = identity.key()?;
    let link = Identity {
        provider,
        subject,
        account_id: account.id,
        created_at: state.now(),
    };
    match state.store().insert_identity(&link).await? {
        Insertion::Inserted => audit_link(state, account, &link).await,
        Insertion::Existing(owner) if owner == account.id => Ok(()),
        Insertion::Existing(_) => Err(ApiError::new(
            axum::http::StatusCode::CONFLICT,
            ProblemKind::IDENTITY_LINKED_ELSEWHERE,
            "this sign-in already belongs to another account",
        )),
    }
}

async fn existing(state: &AppState, account: AccountId) -> ApiResult<Linked> {
    let account = state
        .store()
        .account(account)
        .await?
        .ok_or_else(|| ApiError::internal("a linked account vanished"))?;
    Ok(Linked {
        account,
        created: false,
    })
}

/// Links are recorded with the account as the actor: it is signing itself in.
async fn audit_link(state: &AppState, account: &Account, link: &Identity) -> ApiResult<()> {
    let actor = Principal::Account {
        id: account.id,
        role: account.role,
        organizations: Vec::new(),
    };
    state
        .audit(
            &actor,
            "identity.link",
            format!("{}/{}", account.id, link.provider),
        )
        .await
}
