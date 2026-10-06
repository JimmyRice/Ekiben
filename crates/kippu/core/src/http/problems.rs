//! Errors the framework answers become problems too.
//!
//! An unknown route, a wrong method, a path or query parameter that does not parse and a
//! panicking handler are answered by axum, tower or their layers with plain text or nothing.
//! [`framework_errors`] turns each into the same `application/problem+json` every other error
//! uses, so a client handles one error format:
//!
//! | Cause | Status | Problem kind |
//! |---|---|---|
//! | no route matches | 404 | `not-found` |
//! | the route does not take the method | 405 | `method-not-allowed` |
//! | a path or query parameter that does not parse | 400 | `invalid-parameter` |
//! | a handler panicked | 500 | `internal` |
//! | the request timed out | 408 | `request-timeout` |
//!
//! Any other status keeps its reason phrase as the kind. Headers such as `Allow` are kept.

use axum::body::{HttpBody as _, to_bytes};
use axum::extract::Request;
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::Response;

use crate::error::Problem;

/// The longest rejection text worth reading back as the problem's detail.
const REJECTION_TEXT: usize = 512;

/// Rewrites error responses that are not already problems.
pub(crate) async fn framework_errors(request: Request, next: Next) -> Response {
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
    let text = match body.size_hint().exact() {
        Some(size) if size > 0 && size <= REJECTION_TEXT as u64 => to_bytes(body, REJECTION_TEXT)
            .await
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_default(),
        _ => String::new(),
    };
    let (kind, detail) = describe(status, text);
    kippu_telemetry::record_problem(&kind, &detail, None);
    let mut rewritten = Problem::response(status, &kind, detail);
    // Keep what the framework added (`Allow`, …), but not what described the old body.
    for (name, value) in &parts.headers {
        if name != header::CONTENT_TYPE && name != header::CONTENT_LENGTH {
            rewritten.headers_mut().insert(name.clone(), value.clone());
        }
    }
    rewritten
}

/// The problem kind and detail for an error status and the text the framework gave, if any.
fn describe(status: StatusCode, text: String) -> (String, String) {
    let reason = status.canonical_reason().unwrap_or("error");
    let kind = match status {
        StatusCode::BAD_REQUEST if text.starts_with("Invalid URL") || text.contains("query") => {
            "invalid-parameter".to_owned()
        }
        StatusCode::INTERNAL_SERVER_ERROR => "internal".to_owned(),
        _ => reason.to_ascii_lowercase().replace(' ', "-"),
    };
    let detail = match (status, text.is_empty()) {
        (StatusCode::INTERNAL_SERVER_ERROR, _) => "internal error".to_owned(),
        (StatusCode::NOT_FOUND, true) => "no route matches this path".to_owned(),
        (_, true) => reason.to_owned(),
        (_, false) => text,
    };
    (kind, detail)
}
