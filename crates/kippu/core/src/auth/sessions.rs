//! Signing accounts in: access tokens plus single-use refresh tokens.
//!
//! [`issue`] is how *any* way of signing in ends — the built-in password login, or a module
//! of yours that verified someone through an external provider (see [`super::external`]).

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use kippu_domain::account::Account;
use kippu_domain::{Duration, SessionId, Timestamp};
use kippu_store::{Session, SessionRenewal};
use serde::Serialize;
use sha2::{Digest, Sha256};
use utoipa::ToSchema;

use crate::app::AppState;
use crate::auth::tokens::IssuedToken;
use crate::error::{ApiError, ApiResult};

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

/// Signs `account` in: issues an access token and starts a refresh-token session.
///
/// Call it once you know who someone is, however you found out.
pub async fn issue(state: &AppState, account: Account) -> ApiResult<SessionResponse> {
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
pub async fn refresh(state: &AppState, refresh_token: &str) -> ApiResult<SessionResponse> {
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

/// Ends the session holding `refresh_token`, if any. Access tokens lapse on their own.
pub async fn revoke(state: &AppState, refresh_token: &str) -> ApiResult<()> {
    Ok(state
        .store()
        .revoke_session(&hash_token(refresh_token))
        .await?)
}
