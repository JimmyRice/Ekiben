//! API errors, rendered as RFC 9457 `application/problem+json`.

use std::borrow::Cow;

use axum::Json;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use kippu_domain::ValidationError;
use kippu_domain::reservation::IllegalTransition;
use kippu_store::{BoxError, StoreError};
use kippu_telemetry::Origin;
use serde::Serialize;

/// The status codes problems are reported with, re-exported so services build their errors
/// without depending on the HTTP framework.
pub use axum::http::StatusCode;

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
    origin: Origin,
}

impl ApiError {
    /// An error with a custom problem kind.
    #[track_caller]
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
            origin: Origin::capture(),
        }
    }

    /// 401: no or invalid credentials.
    #[track_caller]
    pub fn unauthenticated(detail: impl Into<Cow<'static, str>>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "unauthenticated", detail)
    }

    /// 403: authenticated, but not allowed.
    #[track_caller]
    pub fn forbidden() -> Self {
        Self::new(
            StatusCode::FORBIDDEN,
            "forbidden",
            "you are not allowed to do this",
        )
    }

    /// 404: `what` does not exist (or is not visible to the caller).
    #[track_caller]
    pub fn not_found(what: &'static str) -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            "not-found",
            format!("{what} not found"),
        )
    }

    /// 409: the request conflicts with the current state of `what`.
    #[track_caller]
    pub fn conflict(what: &'static str) -> Self {
        Self::new(
            StatusCode::CONFLICT,
            "conflict",
            format!("conflict on {what}"),
        )
    }

    /// 413: the request body is larger than `server.max_body_bytes`.
    #[track_caller]
    pub fn payload_too_large() -> Self {
        Self::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "payload-too-large",
            "request body too large",
        )
    }

    /// 412: the record changed since the caller read it.
    #[track_caller]
    pub fn stale_version() -> Self {
        Self::new(
            StatusCode::PRECONDITION_FAILED,
            "stale-version",
            "the record was changed by someone else; fetch it again",
        )
    }

    /// 503: a dependency is temporarily unavailable. Safe to retry.
    #[track_caller]
    pub fn unavailable(source: impl Into<BoxError>) -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "temporarily unavailable; retry later",
        )
        .with_source(source)
    }

    /// 500: a bug or an unexpected failure. Details are logged, not returned.
    #[track_caller]
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

    /// Where the error was raised, for logs.
    pub const fn origin(&self) -> Origin {
        self.origin
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
    #[track_caller]
    fn from(error: ValidationError) -> Self {
        Self::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid-request",
            error.to_string(),
        )
    }
}

impl From<IllegalTransition> for ApiError {
    #[track_caller]
    fn from(error: IllegalTransition) -> Self {
        Self::new(
            StatusCode::CONFLICT,
            "illegal-transition",
            error.to_string(),
        )
    }
}

impl From<StoreError> for ApiError {
    #[track_caller]
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

impl Problem {
    /// The `application/problem+json` response for a problem of `kind`.
    pub(crate) fn response(status: StatusCode, kind: &str, detail: String) -> Response {
        let problem = Self {
            kind: format!("urn:kippu:problem:{kind}"),
            title: status.canonical_reason().unwrap_or("Error").to_owned(),
            status: status.as_u16(),
            detail,
        };
        let mut response = (status, Json(problem)).into_response();
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static("application/problem+json"),
        );
        response
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        crate::http::trace::record_problem(&self);
        if self.status.is_server_error() {
            tracing::error!(error = %self, source = ?self.source, origin = %self.origin, "request failed");
        }
        Problem::response(self.status, self.kind, self.detail.into_owned())
    }
}
