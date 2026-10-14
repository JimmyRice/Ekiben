//! What signed-in callers manage themselves: their name and contact details, email, password
//! and external sign-ins, and closing their account.

use kippu_domain::account::{Account, Contacts, Identity, validate_contacts};
use kippu_domain::validation::{Email, ProviderName, Subject};
use kippu_domain::{AccountId, OrganizationId};
use kippu_store::Unlink;

use crate::app::AppState;
use crate::auth::password::{hash_password, validate_password, verify_password};
use crate::auth::{Principal, Scope};
use crate::error::{ApiError, ApiResult, ProblemKind, StatusCode};
use crate::modules::accounts::permissions::PROFILE_MANAGE;

use super::new_account::validate_display_name;

/// Changes to the caller's profile. `None` keeps the stored value.
#[derive(Debug, Clone, Default)]
pub struct ProfileChanges {
    /// Name shown to others.
    pub display_name: Option<String>,
    /// Ways to reach the person, replaced as a whole.
    pub contacts: Option<Contacts>,
}

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

/// The caller's own account, if it is still open; a closed one is reported as missing.
async fn open_account(state: &AppState, principal: &Principal) -> ApiResult<Account> {
    let id = own_account(state, principal)?;
    state
        .store()
        .account(id)
        .await?
        .filter(|account| !account.is_closed())
        .ok_or_else(|| ApiError::not_found("account"))
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
    check_current_password(state, account, current).await?;
    let hash = hash_password(new).await?;
    if state.store().set_password_hash(account, &hash).await? {
        Ok(())
    } else {
        Err(ApiError::not_found("account"))
    }
}

/// Fails with `403 wrong-password` unless `current` is the account's password. An account
/// without one needs none.
async fn check_current_password(
    state: &AppState,
    account: AccountId,
    current: Option<String>,
) -> ApiResult<()> {
    let Some(hash) = state.store().password_hash(account).await? else {
        return Ok(());
    };
    let matches = match current {
        Some(current) => verify_password(current, hash).await?,
        None => false,
    };
    if matches {
        Ok(())
    } else {
        Err(ApiError::new(
            StatusCode::FORBIDDEN,
            ProblemKind::WRONG_PASSWORD,
            "the current password is missing or wrong",
        ))
    }
}

/// Changes the caller's name or contact details.
#[tracing::instrument(skip_all)]
pub async fn update_profile(
    state: &AppState,
    principal: &Principal,
    changes: ProfileChanges,
) -> ApiResult<Account> {
    let current = open_account(state, principal).await?;
    let account = Account {
        display_name: changes.display_name.unwrap_or(current.display_name),
        contacts: changes.contacts.unwrap_or(current.contacts),
        ..current
    };
    validate_display_name(&account.display_name)?;
    validate_contacts(&account.contacts)?;
    if !state
        .store()
        .update_profile(account.id, &account.display_name, &account.contacts)
        .await?
    {
        return Err(ApiError::not_found("account"));
    }
    Ok(account)
}

/// Closes the caller's own account; one with a password must give it. Everything about the
/// person goes, everything recorded under the id stays, and neither the email nor the
/// external sign-ins can make another account.
#[tracing::instrument(skip_all)]
pub async fn close_own_account(
    state: &AppState,
    principal: &Principal,
    current_password: Option<String>,
) -> ApiResult<()> {
    let account = open_account(state, principal).await?;
    check_current_password(state, account.id, current_password).await?;
    close(state, principal, &account).await
}

/// Closes an account: everything about the person goes, everything recorded under the id
/// (orders, tickets, refunds, denials) stays, and neither its email nor its external sign-ins
/// can ever make another account. Access tokens already issued lapse on their own. Closing a
/// closed account changes nothing.
pub(super) async fn close(
    state: &AppState,
    principal: &Principal,
    account: &Account,
) -> ApiResult<()> {
    let fingerprint = account.email.as_ref().map(Email::fingerprint);
    if state
        .store()
        .close_account(account.id, fingerprint.as_deref(), state.now())
        .await?
    {
        state.audit(principal, "account.close", account.id).await?;
        tracing::info!(account = %account.id, "account closed");
    }
    Ok(())
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
    // A closed account keeps its identities, so they never make another account.
    let account = open_account(state, principal).await?.id;
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
            ProblemKind::LAST_SIGN_IN_METHOD,
            "this is the account's only way to sign in; set a password first",
        )),
    }
}
