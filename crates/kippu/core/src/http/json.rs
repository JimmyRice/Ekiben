//! A JSON extractor whose rejections are Kippu problems.

use axum::extract::rejection::JsonRejection;
use axum::extract::{FromRequest, Request};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::ApiError;

/// A JSON request or response body.
///
/// Works like [`axum::Json`], but a body that cannot be read answers with an
/// `application/problem+json` response instead of axum's plain text:
///
/// | Cause | Status | Problem kind |
/// |---|---|---|
/// | not JSON, or JSON of the wrong shape (types, missing or unknown fields) | 400 / 422 | `invalid-json` |
/// | no `Content-Type: application/json` | 415 | `unsupported-media-type` |
/// | larger than `server.max_body_bytes` | 413 | `payload-too-large` |
#[derive(Debug, Clone, Copy, Default)]
pub struct Json<T>(pub T);

impl<T, S> FromRequest<S> for Json<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(request: Request, state: &S) -> Result<Self, ApiError> {
        match axum::Json::<T>::from_request(request, state).await {
            Ok(axum::Json(value)) => Ok(Self(value)),
            Err(rejection) => Err(problem(&rejection)),
        }
    }
}

fn problem(rejection: &JsonRejection) -> ApiError {
    let status = rejection.status();
    let kind = match rejection {
        JsonRejection::MissingJsonContentType(_) => "unsupported-media-type",
        _ if status == StatusCode::PAYLOAD_TOO_LARGE => return ApiError::payload_too_large(),
        _ => "invalid-json",
    };
    ApiError::new(status, kind, rejection.body_text())
}

impl<T: Serialize> IntoResponse for Json<T> {
    fn into_response(self) -> Response {
        axum::Json(self.0).into_response()
    }
}
