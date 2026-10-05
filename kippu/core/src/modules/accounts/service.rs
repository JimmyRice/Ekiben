//! Account use cases, independent of HTTP.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use kippu_domain::account::Account;
use kippu_domain::validation::non_empty;
use kippu_domain::{Duration, SessionId};
use kippu_store::{Session, SessionRenewal};
use sha2::{Digest, Sha256};

use super::dto::SessionResponse;
use crate::app::AppState;
use crate::error::{ApiError, ApiResult};

/// A random, URL-safe token with 256 bits of entropy.
pub(crate) fn random_token() -> ApiResult<String> {
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes).map_err(|error| ApiError::internal(error.to_string()))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

/// Refresh tokens are stored only as hashes.
pub(crate) fn hash_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

pub(crate) fn validate_display_name(display_name: &str) -> ApiResult<()> {
    Ok(non_empty("display_name", display_name, 100)?)
}

/// Issues an access token and a fresh refresh-token session for `account`.
pub(crate) async fn start_session(
    state: &AppState,
    account: Account,
) -> ApiResult<SessionResponse> {
    let now = state.now();
    let refresh_token = random_token()?;
    let ttl = Duration::seconds(i64::from(state.config().auth.refresh_token_ttl_seconds));
    let session = Session {
        id: SessionId::generate(),
        account_id: account.id,
        refresh_token_hash: hash_token(&refresh_token),
        created_at: now,
        expires_at: now + ttl,
    };
    state.store().insert_session(&session).await?;
    let organizations = state.store().memberships(account.id).await?;
    Ok(SessionResponse {
        access_token: state.tokens().issue_access(&account, organizations, now),
        refresh_token,
        refresh_expires_at: session.expires_at,
        account,
    })
}

/// Rotates a refresh token: the old one stops working, a new session takes its place.
pub(crate) async fn refresh_session(
    state: &AppState,
    refresh_token: &str,
) -> ApiResult<SessionResponse> {
    let now = state.now();
    let next_token = random_token()?;
    let ttl = Duration::seconds(i64::from(state.config().auth.refresh_token_ttl_seconds));
    let next = SessionRenewal {
        id: SessionId::generate(),
        refresh_token_hash: hash_token(&next_token),
        created_at: now,
        expires_at: now + ttl,
    };
    let account_id = state
        .store()
        .rotate_session(&hash_token(refresh_token), &next)
        .await?
        .ok_or_else(|| {
            ApiError::unauthenticated("refresh token is invalid, expired or already used")
        })?;
    let account = state
        .store()
        .account(account_id)
        .await?
        .ok_or_else(|| ApiError::unauthenticated("account no longer exists"))?;
    let organizations = state.store().memberships(account.id).await?;
    Ok(SessionResponse {
        access_token: state.tokens().issue_access(&account, organizations, now),
        refresh_token: next_token,
        refresh_expires_at: next.expires_at,
        account,
    })
}
