//! What logs know about each request and each background task run.
//!
//! The span contract itself lives in `kippu-telemetry`; this file fills it in from what only
//! core knows: the HTTP request and the [`Principal`]. Every request runs inside a
//! [`REQUEST_SPAN`](kippu_telemetry::REQUEST_SPAN) span and every run of a background task
//! inside a [`TASK_SPAN`](kippu_telemetry::TASK_SPAN) span. Service functions are instrumented
//! with `#[tracing::instrument(skip_all)]`: the spans they open nest inside the request or
//! task run and make up its call chain.
//!
//! Headers such as `Authorization` and request bodies are never recorded.

use std::fmt::Display;
use std::net::SocketAddr;
use std::time::Duration;

use axum::body::HttpBody as _;
use axum::body::{Body, to_bytes};
use axum::extract::{ConnectInfo, Request};
use axum::http::{Response, header};
use axum::middleware::Next;
use kippu_domain::AttestorId;
use tracing::Span;

use super::idempotency::IDEMPOTENCY_KEY;
use crate::auth::Principal;
use crate::error::ApiError;

/// The span for one request. `x-request-id` must already be set.
pub(crate) fn make_span(request: &Request) -> Span {
    let header = |name: &str| {
        request
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
    };
    let client = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(address)| address as &dyn Display);
    kippu_telemetry::request_span(
        request.method(),
        request.uri().path(),
        header("x-request-id").unwrap_or_default(),
        client,
        header(IDEMPOTENCY_KEY),
    )
}

/// Records how the request ended.
pub(crate) fn on_response<B>(response: &Response<B>, latency: Duration, span: &Span) {
    kippu_telemetry::record_response(span, response.status().as_u16(), latency);
}

/// Records who is calling, on the current request's span.
pub(crate) fn record_caller(principal: &Principal) {
    match principal {
        Principal::Root { .. } => kippu_telemetry::record_caller(principal.actor()),
        Principal::Account { role, .. } => {
            kippu_telemetry::record_caller(format_args!(
                "{} ({})",
                principal.actor(),
                role.as_str()
            ));
        }
    }
}

/// Records that an attestor signed the current request.
pub(crate) fn record_attestor(id: AttestorId) {
    kippu_telemetry::record_caller(format_args!("attestor:{id}"));
}

/// Records an error response's problem, on the current request's span.
pub(crate) fn record_problem(error: &ApiError) {
    kippu_telemetry::record_problem(error.kind(), error.detail(), Some(error.origin()));
}

/// A background task's span. `outcome` is recorded with [`record_outcome`].
pub(crate) fn task_span(name: &'static str) -> Span {
    kippu_telemetry::task_span(name)
}

/// Records how a run of a background task ended.
pub(crate) fn record_outcome(span: &Span, outcome: &'static str) {
    kippu_telemetry::record_outcome(span, outcome);
}

/// The longest rejection text worth reading back for the log.
const REJECTION_TEXT: usize = 512;

/// Explains in the log the error responses that are not [`ApiError`]s — an unknown route, a
/// wrong method, a path parameter that does not parse — which would otherwise show a bare
/// status. The response itself is passed on unchanged.
pub(crate) async fn explain_rejections(request: Request, next: Next) -> Response<Body> {
    let response = next.run(request).await;
    let status = response.status();
    let is_problem = response
        .headers()
        .get(header::CONTENT_TYPE)
        .is_some_and(|value| value.as_bytes().starts_with(b"application/problem+json"));
    if !(status.is_client_error() || status.is_server_error()) || is_problem {
        return response;
    }
    let (parts, body) = response.into_parts();
    let readable = body
        .size_hint()
        .exact()
        .is_some_and(|size| size <= REJECTION_TEXT as u64);
    let (body, text) = if readable {
        match to_bytes(body, REJECTION_TEXT).await {
            Ok(bytes) => {
                let text = String::from_utf8_lossy(&bytes).into_owned();
                (Body::from(bytes), text)
            }
            Err(_) => (Body::empty(), String::new()),
        }
    } else {
        (body, String::new())
    };
    let kind = status
        .canonical_reason()
        .unwrap_or("error")
        .to_ascii_lowercase()
        .replace(' ', "-");
    let detail = if !text.is_empty() {
        text
    } else if status == axum::http::StatusCode::NOT_FOUND {
        "no route matches this path".to_owned()
    } else {
        status.canonical_reason().unwrap_or("error").to_owned()
    };
    kippu_telemetry::record_problem(&kind, &detail, None);
    Response::from_parts(parts, body)
}
