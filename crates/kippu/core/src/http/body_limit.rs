//! Request body limits: the deployment's default, and larger ones modules declare.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::header;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use http_body_util::Limited;

use crate::error::ApiError;
use crate::module::BodyLimit;

/// `server.max_body_bytes`, and the larger limits modules declare for some routes.
pub(crate) struct BodyLimits {
    pub(crate) default: usize,
    pub(crate) routes: Vec<BodyLimit>,
}

/// The body limit that applies to a request, for code that buffers bodies itself.
#[derive(Debug, Clone, Copy)]
pub struct RequestBodyLimit(pub usize);

/// Enforces the body limit for the request's route: a declared `Content-Length` over it is
/// answered with a 413 problem at once, and the body is cut off when it grows past it, which
/// extractors report as 413 too.
pub(crate) async fn limit_body(
    State(limits): State<Arc<BodyLimits>>,
    mut request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path();
    let limit = limits
        .routes
        .iter()
        .find(|route| route.matches(path))
        .map_or(limits.default, |route| route.max_bytes);
    let declared = request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok()?.parse::<usize>().ok());
    if declared.is_some_and(|length| length > limit) {
        return ApiError::payload_too_large().into_response();
    }
    request.extensions_mut().insert(RequestBodyLimit(limit));
    let request = request.map(|body| Body::new(Limited::new(body, limit)));
    next.run(request).await
}
