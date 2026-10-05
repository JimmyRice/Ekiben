//! API errors, rendered as RFC 9457 `application/problem+json`.

use std::borrow::Cow;

use axum::Json;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use kippu_domain::ValidationError;
use kippu_domain::reservation::IllegalTransition;
use kippu_store::{BoxError, StoreError};
use serde::Serialize;

/// Result of an API operation.
pub type ApiResult<T> = Result<T, ApiError>;

/// An error returned to an API client.
///
/// `kind` becomes the problem type `urn:kippu:problem:<kind>`: a stable identifier clients
/// can match on, unlike `detail`, which is for humans. Modules create their own kinds with
/// [`ApiError::new`].
#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    kind: &'static str,
    detail: Cow<'static, str>,
    source: Option<BoxError>,
}

impl ApiError {
    /// An error with a custom problem kind.
    pub fn new(
        status: StatusCode,
        kind: &'static str,
        detail: impl Into<Cow<'static, str>>,
    ) -> Self {
        Self {
            status,
            kind,
            detail: detail.into(),
            source: None,
        }
    }

    /// 401: no or invalid credentials.
    pub fn unauthenticated(detail: impl Into<Cow<'static, str>>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "unauthenticated", detail)
    }

    /// 403: authenticated, but not allowed.
    pub fn forbidden() -> Self {
        Self::new(
            StatusCode::FORBIDDEN,
            "forbidden",
            "you are not allowed to do this",
        )
    }

    /// 404: `what` does not exist (or is not visible to the caller).
    pub fn not_found(what: &'static str) -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            "not-found",
            format!("{what} not found"),
        )
    }

    /// 409: the request conflicts with the current state of `what`.
    pub fn conflict(what: &'static str) -> Self {
        Self::new(
            StatusCode::CONFLICT,
            "conflict",
            format!("conflict on {what}"),
        )
    }

    /// 412: the record changed since the caller read it.
    pub fn stale_version() -> Self {
        Self::new(
            StatusCode::PRECONDITION_FAILED,
            "stale-version",
            "the record was changed by someone else; fetch it again",
        )
    }

    /// 503: a dependency is temporarily unavailable. Safe to retry.
    pub fn unavailable(source: impl Into<BoxError>) -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "temporarily unavailable; retry later",
        )
        .with_source(source)
    }

    /// 500: a bug or an unexpected failure. Details are logged, not returned.
    pub fn internal(source: impl Into<BoxError>) -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "internal error",
        )
        .with_source(source)
    }

    /// Attaches the underlying cause, for logs.
    #[must_use]
    pub fn with_source(mut self, source: impl Into<BoxError>) -> Self {
        self.source = Some(source.into());
        self
    }

    /// The HTTP status.
    pub const fn status(&self) -> StatusCode {
        self.status
    }

    /// The problem kind, e.g. `"sold-out"`.
    pub const fn kind(&self) -> &'static str {
        self.kind
    }

    /// The human-readable explanation.
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({}): {}", self.kind, self.status, self.detail)
    }
}

impl std::error::Error for ApiError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source.as_deref().map(|source| source as _)
    }
}

impl From<ValidationError> for ApiError {
    fn from(error: ValidationError) -> Self {
        Self::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid-request",
            error.to_string(),
        )
    }
}

impl From<IllegalTransition> for ApiError {
    fn from(error: IllegalTransition) -> Self {
        Self::new(
            StatusCode::CONFLICT,
            "illegal-transition",
            error.to_string(),
        )
    }
}

impl From<StoreError> for ApiError {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::Conflict("version") => Self::stale_version(),
            StoreError::Conflict(what) => Self::conflict(what),
            StoreError::NotFound(what) => Self::not_found(what),
            StoreError::Unavailable(source) => Self::unavailable(source),
            StoreError::Backend(source) => Self::internal(source),
        }
    }
}

/// The JSON body of a problem response.
#[derive(Serialize, utoipa::ToSchema)]
pub struct Problem {
    /// Stable identifier of the kind of problem, e.g. `urn:kippu:problem:sold-out`.
    #[serde(rename = "type")]
    pub kind: String,
    /// Short summary of the HTTP status.
    pub title: String,
    /// HTTP status code.
    pub status: u16,
    /// Human-readable explanation.
    pub detail: String,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        crate::http::trace::record_problem(&self);
        if self.status.is_server_error() {
            tracing::error!(error = %self, source = ?self.source, "request failed");
        }
        let problem = Problem {
            kind: format!("urn:kippu:problem:{}", self.kind),
            title: self.status.canonical_reason().unwrap_or("Error").to_owned(),
            status: self.status.as_u16(),
            detail: self.detail.into_owned(),
        };
        let mut response = (self.status, Json(problem)).into_response();
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static("application/problem+json"),
        );
        response
    }
}
