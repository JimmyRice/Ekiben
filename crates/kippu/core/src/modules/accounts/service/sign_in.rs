//! Signing up and signing in with an email and password.

use kippu_domain::account::{Account, Role};
use kippu_domain::validation::Email;

use super::{NewAccount, insert_account};
use crate::app::AppState;
use crate::auth::password::{DUMMY_HASH, verify_password};
use crate::auth::sessions::{self, SessionResponse};
use crate::error::{ApiError, ApiResult};

/// Signs a new `user` up.
#[tracing::instrument(skip_all)]
pub async fn register(state: &AppState, new: NewAccount) -> ApiResult<Account> {
    insert_account(state, new, Role::User).await
}

/// Signs in with an email and password, starting a session.
#[tracing::instrument(skip_all)]
pub async fn login(
    state: &AppState,
    email: String,
    password: String,
) -> ApiResult<SessionResponse> {
    let invalid = || ApiError::unauthenticated("wrong email or password");
    let credentials = match Email::new(email) {
        Ok(email) => state.store().account_credentials(&email).await?,
        Err(_) => None,
    };
    // Verify against a dummy hash when there is no account or it has no password (it signs
    // in through an external provider), so timing reveals nothing.
    let (account, hash) = match credentials {
        Some((account, Some(hash))) => (Some(account), hash),
        _ => (None, DUMMY_HASH.clone()),
    };
    let password_matches = verify_password(password, hash).await?;
    match account {
        Some(account) if password_matches => sessions::issue(state, account).await,
        _ => Err(invalid()),
    }
}
