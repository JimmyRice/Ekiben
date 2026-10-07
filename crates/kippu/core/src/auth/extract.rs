//! Taking the [`Principal`] from a request's `Authorization: Bearer` header.

use axum::extract::{FromRequestParts, OptionalFromRequestParts};
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;

use super::Principal;
use crate::app::AppState;
use crate::error::ApiError;

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
