//! What signed-in callers manage themselves: their email, password and external sign-ins.

use kippu_domain::account::{Account, Identity};
use kippu_domain::validation::{Email, ProviderName, Subject};
use kippu_domain::{AccountId, OrganizationId};
use kippu_store::Unlink;

use crate::app::AppState;
use crate::auth::password::{hash_password, validate_password, verify_password};
use crate::auth::{Principal, Scope};
use crate::error::{ApiError, ApiResult, StatusCode};
use crate::modules::accounts::permissions::PROFILE_MANAGE;

/// Who a caller is.
#[derive(Debug, Clone)]
pub enum Whoami {
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

/// The caller's own account, after checking they may manage it.
fn own_account(state: &AppState, principal: &Principal) -> ApiResult<AccountId> {
    let account = principal.require_account()?;
    state.authorize(principal, PROFILE_MANAGE, Scope::Account(account))?;
    Ok(account)
}

/// Who the caller is.
#[tracing::instrument(skip_all)]
pub async fn whoami(state: &AppState, principal: &Principal) -> ApiResult<Whoami> {
    match principal {
        Principal::Root { key_name } => Ok(Whoami::Root {
            key_name: key_name.clone(),
        }),
        Principal::Account { id, .. } => {
            let account = state
                .store()
                .account(*id)
                .await?
                .ok_or_else(|| ApiError::not_found("account"))?;
            let organizations = state.store().memberships(*id).await?;
            Ok(Whoami::Account {
                account,
                organizations,
            })
        }
    }
}

/// Sets the caller's email address, e.g. after signing up through a provider that shared
/// none.
#[tracing::instrument(skip_all)]
pub async fn set_email(
    state: &AppState,
    principal: &Principal,
    email: String,
) -> ApiResult<Account> {
    let account = own_account(state, principal)?;
    let email = Email::new(email)?;
    if !state.store().set_email(account, &email).await? {
        return Err(ApiError::not_found("account"));
    }
    state
        .store()
        .account(account)
        .await?
        .ok_or_else(|| ApiError::not_found("account"))
}

/// Sets or changes an account's password. Changing one requires the current password;
/// accounts created through an external sign-in set their first one without.
#[tracing::instrument(skip_all)]
pub async fn set_password(
    state: &AppState,
    account: AccountId,
    current: Option<String>,
    new: String,
) -> ApiResult<()> {
    validate_password(&new)?;
    if let Some(hash) = state.store().password_hash(account).await? {
        let matches = match current {
            Some(current) => verify_password(current, hash).await?,
            None => false,
        };
        if !matches {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "wrong-password",
                "the current password is missing or wrong",
            ));
        }
    }
    let hash = hash_password(new).await?;
    if state.store().set_password_hash(account, &hash).await? {
        Ok(())
    } else {
        Err(ApiError::not_found("account"))
    }
}

/// Sets or changes the caller's own password, and records that in the audit log.
#[tracing::instrument(skip_all)]
pub async fn change_password(
    state: &AppState,
    principal: &Principal,
    current: Option<String>,
    new: String,
) -> ApiResult<()> {
    let account = own_account(state, principal)?;
    set_password(state, account, current, new).await?;
    state
        .audit(principal, "account.password.set", account)
        .await
}

/// The external sign-ins linked to the caller's account.
#[tracing::instrument(skip_all)]
pub async fn identities(state: &AppState, principal: &Principal) -> ApiResult<Vec<Identity>> {
    let account = own_account(state, principal)?;
    Ok(state.store().identities(account).await?)
}

/// Unlinks one of the caller's external sign-ins. An account always keeps a way to sign in:
/// the last one cannot be unlinked until a password is set.
#[tracing::instrument(skip_all)]
pub async fn unlink_identity(
    state: &AppState,
    principal: &Principal,
    provider: String,
    subject: String,
) -> ApiResult<()> {
    let account = own_account(state, principal)?;
    let not_found = || ApiError::not_found("identity");
    let provider = ProviderName::new(provider).map_err(|_| not_found())?;
    let subject = Subject::new(subject).map_err(|_| not_found())?;
    match state
        .store()
        .unlink_identity(account, &provider, &subject)
        .await?
    {
        Unlink::Unlinked => {
            state
                .audit(
                    principal,
                    "identity.unlink",
                    format!("{account}/{provider}"),
                )
                .await
        }
        Unlink::NotLinked => Err(not_found()),
        Unlink::LastSignInMethod => Err(ApiError::new(
            StatusCode::CONFLICT,
            "last-sign-in-method",
            "this is the account's only way to sign in; set a password first",
        )),
    }
}
