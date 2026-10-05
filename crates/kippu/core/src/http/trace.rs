//! What logs know about each request and each background task run.
//!
//! Every request runs inside a [`REQUEST_SPAN`] span and every run of a background task inside
//! a [`TASK_SPAN`] span. Handlers and services log events as usual; events end up attached to
//! the request or run that caused them, so a log layer can render each as one unit (see
//! `kippu-server`'s log formats).
//!
//! The spans carry only the fields listed here. Headers such as `Authorization` and request
//! bodies are never recorded.

use std::net::SocketAddr;
use std::time::Duration;

use axum::extract::{ConnectInfo, Request};
use axum::http::Response;
use tracing::Span;
use tracing::field::{Empty, display};

use super::idempotency::IDEMPOTENCY_KEY;
use kippu_domain::AttestorId;

use crate::auth::Principal;
use crate::error::ApiError;

/// Name of the span each request runs in.
///
/// Fields: `method`, `path` (without the query), `request_id`, `client` (the peer address,
/// when known), `idempotency_key`, and — recorded as the request progresses — `caller`
/// ([`Principal::actor`] plus the role, or `attestor:<id>`), `problem` (the problem kind and detail of an error
/// response), `status` and `latency_us`.
pub const REQUEST_SPAN: &str = "request";

/// Name of the span each run of a background task runs in.
///
/// Fields: `task` (its name) and `outcome`: `idle`, `more work` or `failed`.
pub const TASK_SPAN: &str = "task";

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
        .map(|ConnectInfo(address)| display(*address));
    tracing::info_span!(
        REQUEST_SPAN,
        method = %request.method(),
        path = request.uri().path(),
        request_id = header("x-request-id").unwrap_or_default(),
        client,
        idempotency_key = header(IDEMPOTENCY_KEY),
        caller = Empty,
        problem = Empty,
        status = Empty,
        latency_us = Empty,
    )
}

/// Records how the request ended.
pub(crate) fn on_response<B>(response: &Response<B>, latency: Duration, span: &Span) {
    span.record("status", response.status().as_u16());
    span.record(
        "latency_us",
        u64::try_from(latency.as_micros()).unwrap_or(u64::MAX),
    );
}

/// Records who is calling, on the current request's span.
pub(crate) fn record_caller(principal: &Principal) {
    let caller = match principal {
        Principal::Root { .. } => principal.actor(),
        Principal::Account { role, .. } => {
            format!("{} ({})", principal.actor(), role.as_str())
        }
    };
    Span::current().record("caller", caller);
}

/// Records that an attestor signed the current request.
pub(crate) fn record_attestor(id: AttestorId) {
    Span::current().record("caller", display(format_args!("attestor:{id}")));
}

/// Records an error response's problem, on the current request's span.
pub(crate) fn record_problem(error: &ApiError) {
    Span::current().record(
        "problem",
        display(format_args!("{}: {}", error.kind(), error.detail())),
    );
}

/// A background task's span. `outcome` is recorded with [`record_outcome`].
pub(crate) fn task_span(name: &'static str) -> Span {
    tracing::info_span!(TASK_SPAN, task = name, outcome = Empty)
}

/// Records how a run of a background task ended.
pub(crate) fn record_outcome(span: &Span, outcome: &'static str) {
    span.record("outcome", outcome);
}
