//! The spans every layer of Kippu logs through, and the fields they carry.
//!
//! Handlers, services, workers and adapters never format log output; they open these spans
//! and log events inside them, and the log layer attaches each event to the request or task
//! run that caused it. Headers such as `Authorization` and request bodies are never recorded.

use std::fmt::Display;
use std::time::Duration;

use tracing::Span;
use tracing::field::{Empty, display};

use crate::Origin;

/// Name of the span each request runs in.
///
/// Fields: `method`, `path` (without the query), `request_id`, `client` (the peer address,
/// when known), `idempotency_key`, and — recorded as the request progresses — `caller`,
/// `problem` (the kind and detail of an error response), `origin` (where that problem was
/// raised, see [`Origin`]), `status` and `latency_us`.
pub const REQUEST_SPAN: &str = "request";

/// Name of the span each run of a background task runs in.
///
/// Fields: `task` (its name) and `outcome`: `idle`, `more work` or `failed`.
pub const TASK_SPAN: &str = "task";

/// Name of the span around a call to something outside the process, see [`call`](crate::call).
///
/// Fields: `system`, `operation` and, when the call failed, `error`.
pub const DEPENDENCY_SPAN: &str = "dependency";

/// The span for one request.
pub fn request_span(
    method: &dyn Display,
    path: &str,
    request_id: &str,
    client: Option<&dyn Display>,
    idempotency_key: Option<&str>,
) -> Span {
    tracing::info_span!(
        REQUEST_SPAN,
        method = %method,
        path,
        request_id,
        client = client.map(display),
        idempotency_key,
        caller = Empty,
        problem = Empty,
        origin = Empty,
        status = Empty,
        latency_us = Empty,
    )
}

/// Records how the request ended.
pub fn record_response(span: &Span, status: u16, latency: Duration) {
    span.record("status", status);
    span.record(
        "latency_us",
        u64::try_from(latency.as_micros()).unwrap_or(u64::MAX),
    );
}

/// Records who is calling, on the current request's span.
pub fn record_caller(caller: impl Display) {
    Span::current().record("caller", display(caller));
}

/// Records an error response's problem on the current request's span, and where it was raised
/// when that is known (a response the framework made, such as an unknown route, has no origin).
pub fn record_problem(kind: &str, detail: &str, origin: Option<Origin>) {
    let span = Span::current();
    span.record("problem", display(format_args!("{kind}: {detail}")));
    if let Some(origin) = origin {
        span.record("origin", display(origin));
    }
}

/// A background task's span. `outcome` is recorded with [`record_outcome`].
pub fn task_span(name: &'static str) -> Span {
    tracing::info_span!(TASK_SPAN, task = name, outcome = Empty)
}

/// Records how a run of a background task ended.
pub fn record_outcome(span: &Span, outcome: &'static str) {
    span.record("outcome", outcome);
}
