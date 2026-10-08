//! `Idempotency-Key` for every mutating request.
//!
//! When a client sends `Idempotency-Key` with a POST, PUT, PATCH or DELETE, the first
//! *successful* response is stored. Repeating the request with the same key returns the stored
//! response without running the handler again; reusing the key for a *different* request is
//! rejected. A request that failed changed nothing, so retrying it runs it again — the client
//! may have fixed the cause (e.g. obtained an admission pass) in between. Keys are scoped to the
//! caller, so two accounts never collide.
//!
//! Routes that are idempotent by design — purchase requests derive their id from the key,
//! payment attestations are unique per attestor — are declared by their modules
//! ([`Module::idempotent_routes`](crate::Module::idempotent_routes)) and pass straight through:
//! no buffering, no lookup, nothing stored.
//!
//! A caller's token is verified here once; the [`Principal`] is handed on to the handler in the
//! request's extensions rather than verified again.

use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::extract::{Request, State};
use axum::http::header::{AUTHORIZATION, CONTENT_TYPE};
use axum::http::{HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use kippu_domain::ValidationError;
use kippu_domain::validation::IdempotencyKey;
use kippu_store::IdempotencyRecord;
use sha2::{Digest, Sha256};

use crate::app::AppState;
use crate::auth::Principal;
use crate::error::{ApiError, ProblemKind};
use crate::module::IdempotentRoute;

/// The request header.
pub const IDEMPOTENCY_KEY: &str = "idempotency-key";
/// Set on responses replayed from a stored record.
pub const REPLAYED: &str = "idempotent-replayed";

/// What the middleware needs: the application, and the routes it lets through.
pub(crate) struct Idempotency {
    pub(crate) state: AppState,
    pub(crate) by_design: Vec<IdempotentRoute>,
}

pub(crate) async fn middleware(
    State(idempotency): State<Arc<Idempotency>>,
    request: Request,
    next: Next,
) -> Response {
    let mutating = matches!(
        *request.method(),
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    );
    let Some(key) = request.headers().get(IDEMPOTENCY_KEY).filter(|_| mutating) else {
        return next.run(request).await;
    };
    let Some(key) = key
        .to_str()
        .ok()
        .and_then(|key| IdempotencyKey::new(key).ok())
    else {
        return ApiError::from(ValidationError::new(
            "Idempotency-Key",
            "must be 1-255 visible ASCII characters",
        ))
        .into_response();
    };
    let path = request.uri().path();
    if idempotency
        .by_design
        .iter()
        .any(|route| route.matches(request.method(), path))
    {
        return next.run(request).await;
    }
    handle(&idempotency.state, &key, request, next)
        .await
        .unwrap_or_else(IntoResponse::into_response)
}

async fn handle(
    state: &AppState,
    key: &IdempotencyKey,
    request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let (mut parts, body) = request.into_parts();
    let principal = authenticate(state, &parts);
    let scope = principal
        .as_ref()
        .map_or_else(|| "anonymous".to_owned(), Principal::actor);
    if let Some(principal) = principal {
        // A replayed response never reaches the handler, so record the caller here.
        super::trace::record_caller(&principal);
        parts.extensions.insert(principal);
    }
    let limit = parts
        .extensions
        .get::<super::RequestBodyLimit>()
        .map_or(state.config().server.max_body_bytes, |limit| limit.0);
    let body = to_bytes(body, limit)
        .await
        .map_err(|_| ApiError::payload_too_large())?;
    let fingerprint = hex::encode(
        Sha256::new()
            .chain_update(parts.method.as_str())
            .chain_update(b"\0")
            .chain_update(parts.uri.path_and_query().map_or("", |path| path.as_str()))
            .chain_update(b"\0")
            .chain_update(&body)
            .finalize(),
    );

    let store = state.store();
    if let Some(record) = store.idempotency_record(&scope, key.as_str()).await? {
        tracing::info!("replaying the response stored for this Idempotency-Key");
        return Ok(replay(&record, &fingerprint));
    }

    let response = next.run(Request::from_parts(parts, Body::from(body))).await;
    if !response.status().is_success() {
        return Ok(response);
    }

    let (parts, body) = response.into_parts();
    let body = to_bytes(body, usize::MAX)
        .await
        .map_err(ApiError::internal)?;
    let record = IdempotencyRecord {
        fingerprint,
        status: parts.status.as_u16(),
        body: body.to_vec(),
        created_at: state.now(),
    };
    if let Err(error) = store
        .save_idempotency_record(&scope, key.as_str(), &record)
        .await
    {
        tracing::warn!(%error, "could not store idempotency record");
    }
    Ok(Response::from_parts(parts, Body::from(body)))
}

/// Identifies the caller without failing: an invalid token is left for the handler to reject.
fn authenticate(state: &AppState, parts: &axum::http::request::Parts) -> Option<Principal> {
    parts
        .headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .and_then(|token| state.tokens().authenticate(token, state.now()).ok())
}

fn replay(record: &IdempotencyRecord, fingerprint: &str) -> Response {
    if record.fingerprint != fingerprint {
        return ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            ProblemKind::IDEMPOTENCY_KEY_REUSED,
            "this Idempotency-Key was already used for a different request",
        )
        .into_response();
    }
    let status = StatusCode::from_u16(record.status).unwrap_or(StatusCode::OK);
    let mut response = (status, record.body.clone()).into_response();
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(REPLAYED, HeaderValue::from_static("true"));
    response
}
