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

/// What kind of problem an error is: the `<kind>` in the problem type
/// `urn:kippu:problem:<kind>`, lower-case words joined by hyphens.
///
/// Kinds are API — clients match on them — so they are named once, as constants, and never
/// renamed. Kippu's own are listed in [`ProblemKind::ALL`]; a module of your own names its
/// kinds the same way, in a `const`, where a malformed name fails to compile:
///
/// ```
/// use kippu_core::error::ProblemKind;
///
/// const QUEUE_FULL: ProblemKind = ProblemKind::new("queue-full");
/// assert_eq!(QUEUE_FULL.as_str(), "queue-full");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProblemKind(&'static str);

impl ProblemKind {
    /// A kind named `kind`.
    ///
    /// # Panics
    ///
    /// If `kind` is not lower-case ASCII words joined by single hyphens; in a `const`, that
    /// is a compile error.
    pub const fn new(kind: &'static str) -> Self {
        let bytes = kind.as_bytes();
        assert!(!bytes.is_empty(), "a problem kind must not be empty");
        let mut index = 0;
        while index < bytes.len() {
            let byte = bytes[index];
            let hyphen_ok = index > 0 && index + 1 < bytes.len() && bytes[index - 1] != b'-';
            assert!(
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || (byte == b'-' && hyphen_ok),
                "a problem kind is lower-case words joined by hyphens"
            );
            index += 1;
        }
        Self(kind)
    }

    /// The kind, e.g. `"sale-closed"`.
    pub const fn as_str(self) -> &'static str {
        self.0
    }

    /// A waiting room must be passed first.
    pub const ADMISSION_REQUIRED: Self = Self::new("admission-required");
    /// A payment does not match the reservation total.
    pub const AMOUNT_MISMATCH: Self = Self::new("amount-mismatch");
    /// The sale does not accept that payment method.
    pub const ATTESTOR_NOT_ACCEPTED: Self = Self::new("attestor-not-accepted");
    /// The request conflicts with the current state.
    pub const CONFLICT: Self = Self::new("conflict");
    /// Another account has that email.
    pub const EMAIL_ALREADY_REGISTERED: Self = Self::new("email-already-registered");
    /// A sandbox payment method cannot pay for a live sale.
    pub const ENVIRONMENT_MISMATCH: Self = Self::new("environment-mismatch");
    /// Authenticated, but not allowed.
    pub const FORBIDDEN: Self = Self::new("forbidden");
    /// The `Idempotency-Key` was used for a different request.
    pub const IDEMPOTENCY_KEY_REUSED: Self = Self::new("idempotency-key-reused");
    /// The external identity belongs to another account.
    pub const IDENTITY_LINKED_ELSEWHERE: Self = Self::new("identity-linked-elsewhere");
    /// The record's state does not allow this.
    pub const ILLEGAL_TRANSITION: Self = Self::new("illegal-transition");
    /// The deployment has no image storage.
    pub const IMAGES_NOT_CONFIGURED: Self = Self::new("images-not-configured");
    /// A bug or an unexpected failure.
    pub const INTERNAL: Self = Self::new("internal");
    /// The body could not be read.
    pub const INVALID_BODY: Self = Self::new("invalid-body");
    /// Not JSON, or JSON of the wrong shape.
    pub const INVALID_JSON: Self = Self::new("invalid-json");
    /// A path or query parameter does not parse.
    pub const INVALID_PARAMETER: Self = Self::new("invalid-parameter");
    /// A value fails validation.
    pub const INVALID_REQUEST: Self = Self::new("invalid-request");
    /// The account's last way to sign in cannot be removed.
    pub const LAST_SIGN_IN_METHOD: Self = Self::new("last-sign-in-method");
    /// The route does not take the method.
    pub const METHOD_NOT_ALLOWED: Self = Self::new("method-not-allowed");
    /// The sale has no waiting room.
    pub const NO_WAITING_ROOM: Self = Self::new("no-waiting-room");
    /// Missing, or not visible to the caller.
    pub const NOT_FOUND: Self = Self::new("not-found");
    /// Every ticket of the reservation is refunded already.
    pub const NOTHING_TO_REFUND: Self = Self::new("nothing-to-refund");
    /// The body is larger than the route accepts.
    pub const PAYLOAD_TOO_LARGE: Self = Self::new("payload-too-large");
    /// The payment paid for issued tickets; refund them or report a reversal instead.
    pub const PAYMENT_APPLIED: Self = Self::new("payment-applied");
    /// Buyers may not refund these tickets (any more); organizers can.
    pub const REFUND_NOT_ALLOWED: Self = Self::new("refund-not-allowed");
    /// Only refunds of payments taken in person are confirmed by organizers.
    pub const REFUND_NOT_MANUAL: Self = Self::new("refund-not-manual");
    /// The request took too long.
    pub const REQUEST_TIMEOUT: Self = Self::new("request-timeout");
    /// Only a reservation whose tickets were issued can be refunded.
    pub const RESERVATION_NOT_ISSUED: Self = Self::new("reservation-not-issued");
    /// The sale is not open.
    pub const SALE_CLOSED: Self = Self::new("sale-closed");
    /// The record changed since the caller read it.
    pub const STALE_VERSION: Self = Self::new("stale-version");
    /// A ticket is not part of the reservation.
    pub const TICKET_NOT_IN_RESERVATION: Self = Self::new("ticket-not-in-reservation");
    /// A ticket to refund is revoked already.
    pub const TICKET_REVOKED: Self = Self::new("ticket-revoked");
    /// The event has as many images as allowed.
    pub const TOO_MANY_IMAGES: Self = Self::new("too-many-images");
    /// No or invalid credentials.
    pub const UNAUTHENTICATED: Self = Self::new("unauthenticated");
    /// A dependency is temporarily unavailable.
    pub const UNAVAILABLE: Self = Self::new("unavailable");
    /// The body is not `application/json`.
    pub const UNSUPPORTED_MEDIA_TYPE: Self = Self::new("unsupported-media-type");
    /// The deployment cannot sign webhook deliveries.
    pub const WEBHOOKS_NOT_CONFIGURED: Self = Self::new("webhooks-not-configured");
    /// The current password is wrong.
    pub const WRONG_PASSWORD: Self = Self::new("wrong-password");

    /// Every kind Kippu itself reports.
    pub const ALL: [Self; 37] = [
        Self::ADMISSION_REQUIRED,
        Self::AMOUNT_MISMATCH,
        Self::ATTESTOR_NOT_ACCEPTED,
        Self::CONFLICT,
        Self::EMAIL_ALREADY_REGISTERED,
        Self::ENVIRONMENT_MISMATCH,
        Self::FORBIDDEN,
        Self::IDEMPOTENCY_KEY_REUSED,
        Self::IDENTITY_LINKED_ELSEWHERE,
        Self::ILLEGAL_TRANSITION,
        Self::IMAGES_NOT_CONFIGURED,
        Self::INTERNAL,
        Self::INVALID_BODY,
        Self::INVALID_JSON,
        Self::INVALID_PARAMETER,
        Self::INVALID_REQUEST,
        Self::LAST_SIGN_IN_METHOD,
        Self::METHOD_NOT_ALLOWED,
        Self::NO_WAITING_ROOM,
        Self::NOT_FOUND,
        Self::NOTHING_TO_REFUND,
        Self::PAYLOAD_TOO_LARGE,
        Self::PAYMENT_APPLIED,
        Self::REFUND_NOT_ALLOWED,
        Self::REFUND_NOT_MANUAL,
        Self::REQUEST_TIMEOUT,
        Self::RESERVATION_NOT_ISSUED,
        Self::SALE_CLOSED,
        Self::STALE_VERSION,
        Self::TICKET_NOT_IN_RESERVATION,
        Self::TICKET_REVOKED,
        Self::TOO_MANY_IMAGES,
        Self::UNAUTHENTICATED,
        Self::UNAVAILABLE,
        Self::UNSUPPORTED_MEDIA_TYPE,
        Self::WEBHOOKS_NOT_CONFIGURED,
        Self::WRONG_PASSWORD,
    ];
}

impl std::fmt::Display for ProblemKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

/// An error returned to an API client.
///
/// `kind` becomes the problem type `urn:kippu:problem:<kind>`: a stable identifier clients
/// can match on, unlike `detail`, which is for humans. Modules name their own kinds as
/// [`ProblemKind`] constants and raise them with [`ApiError::new`].
#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    kind: ProblemKind,
    detail: Cow<'static, str>,
    source: Option<BoxError>,
    origin: Origin,
}

impl ApiError {
    /// An error with a custom problem kind.
    #[track_caller]
    pub fn new(
        status: StatusCode,
        kind: ProblemKind,
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
        Self::new(
            StatusCode::UNAUTHORIZED,
            ProblemKind::UNAUTHENTICATED,
            detail,
        )
    }

    /// 403: authenticated, but not allowed.
    #[track_caller]
    pub fn forbidden() -> Self {
        Self::new(
            StatusCode::FORBIDDEN,
            ProblemKind::FORBIDDEN,
            "you are not allowed to do this",
        )
    }

    /// 404: `what` does not exist (or is not visible to the caller).
    #[track_caller]
    pub fn not_found(what: &'static str) -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            ProblemKind::NOT_FOUND,
            format!("{what} not found"),
        )
    }

    /// 409: the request conflicts with the current state of `what`.
    #[track_caller]
    pub fn conflict(what: &'static str) -> Self {
        Self::new(
            StatusCode::CONFLICT,
            ProblemKind::CONFLICT,
            format!("conflict on {what}"),
        )
    }

    /// 413: the request body is larger than `server.max_body_bytes`.
    #[track_caller]
    pub fn payload_too_large() -> Self {
        Self::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            ProblemKind::PAYLOAD_TOO_LARGE,
            "request body too large",
        )
    }

    /// 412: the record changed since the caller read it.
    #[track_caller]
    pub fn stale_version() -> Self {
        Self::new(
            StatusCode::PRECONDITION_FAILED,
            ProblemKind::STALE_VERSION,
            "the record was changed by someone else; fetch it again",
        )
    }

    /// 503: a dependency is temporarily unavailable. Safe to retry.
    #[track_caller]
    pub fn unavailable(source: impl Into<BoxError>) -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            ProblemKind::UNAVAILABLE,
            "temporarily unavailable; retry later",
        )
        .with_source(source)
    }

    /// 500: a bug or an unexpected failure. Details are logged, not returned.
    #[track_caller]
    pub fn internal(source: impl Into<BoxError>) -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            ProblemKind::INTERNAL,
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

    /// The problem kind, e.g. `sale-closed`.
    pub const fn kind(&self) -> ProblemKind {
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
            ProblemKind::INVALID_REQUEST,
            error.to_string(),
        )
    }
}

impl From<IllegalTransition> for ApiError {
    #[track_caller]
    fn from(error: IllegalTransition) -> Self {
        Self::new(
            StatusCode::CONFLICT,
            ProblemKind::ILLEGAL_TRANSITION,
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
        Problem::response(self.status, self.kind.as_str(), self.detail.into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Kinds are API: this list changes only by adding to it.
    #[test]
    fn kinds_are_never_renamed() {
        let kinds: Vec<&str> = ProblemKind::ALL.iter().map(|kind| kind.as_str()).collect();
        assert_eq!(
            kinds,
            [
                "admission-required",
                "amount-mismatch",
                "attestor-not-accepted",
                "conflict",
                "email-already-registered",
                "environment-mismatch",
                "forbidden",
                "idempotency-key-reused",
                "identity-linked-elsewhere",
                "illegal-transition",
                "images-not-configured",
                "internal",
                "invalid-body",
                "invalid-json",
                "invalid-parameter",
                "invalid-request",
                "last-sign-in-method",
                "method-not-allowed",
                "no-waiting-room",
                "not-found",
                "nothing-to-refund",
                "payload-too-large",
                "payment-applied",
                "refund-not-allowed",
                "refund-not-manual",
                "request-timeout",
                "reservation-not-issued",
                "sale-closed",
                "stale-version",
                "ticket-not-in-reservation",
                "ticket-revoked",
                "too-many-images",
                "unauthenticated",
                "unavailable",
                "unsupported-media-type",
                "webhooks-not-configured",
                "wrong-password",
            ]
        );
    }

    #[test]
    #[should_panic(expected = "lower-case words joined by hyphens")]
    fn malformed_kinds_are_refused() {
        let _ = ProblemKind::new("Sold_Out");
    }
}
