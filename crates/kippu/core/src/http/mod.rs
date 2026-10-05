//! HTTP assembly: module routes, health checks, OpenAPI and cross-cutting middleware.

pub mod idempotency;
pub mod trace;

use serde::Deserialize;
use utoipa::IntoParams;

use std::sync::Arc;
use std::time::Duration;

use axum::http::{HeaderValue, StatusCode};
use axum::routing::get;
use axum::{Json, Router, middleware};
use tower::ServiceBuilder;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::cors::CorsLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;
use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::app::AppState;
use crate::error::{ApiError, Problem};
use crate::module::Module;

/// Keyset pagination parameters shared by every list endpoint.
#[derive(Debug, Deserialize, IntoParams)]
pub struct PageQuery {
    /// At most this many items (default 50, at most 200).
    pub limit: Option<u32>,
    /// Return items whose id sorts after this one.
    pub after: Option<uuid::Uuid>,
}

impl PageQuery {
    /// The page to fetch from the store.
    pub fn page(&self) -> kippu_store::PageRequest {
        kippu_store::PageRequest {
            limit: self.limit.unwrap_or(50).clamp(1, 200),
            after: self.after,
        }
    }
}

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Kippu",
        description = "Ticketing infrastructure for conventions. Errors are RFC 9457 problems."
    ),
    components(schemas(Problem)),
    modifiers(&BearerAuth)
)]
struct ApiDoc;

struct BearerAuth;

impl Modify for BearerAuth {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "bearer",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .bearer_format("JWT")
                    .build(),
            ),
        );
    }
}

/// Liveness: the process is up.
#[utoipa::path(get, path = "/healthz", tag = "health", responses((status = 200, description = "Alive")))]
async fn healthz() -> &'static str {
    "ok"
}

/// Readiness: the database is reachable.
#[utoipa::path(
    get,
    path = "/readyz",
    tag = "health",
    responses((status = 200, description = "Ready"), (status = 503, body = Problem))
)]
async fn readyz(state: axum::extract::State<AppState>) -> Result<&'static str, ApiError> {
    state.store().ping().await?;
    Ok("ready")
}

pub(crate) fn router(state: &AppState, modules: &[Arc<dyn Module>]) -> Router {
    let api = modules.iter().fold(
        OpenApiRouter::with_openapi(ApiDoc::openapi())
            .routes(routes!(healthz))
            .routes(routes!(readyz)),
        |api, module| api.merge(module.routes()),
    );
    let (router, openapi) = api.split_for_parts();
    let openapi = Arc::new(openapi);

    let server = &state.config().server;
    let cross_cutting = ServiceBuilder::new()
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(trace::make_span)
                .on_request(())
                .on_response(trace::on_response)
                .on_body_chunk(())
                .on_eos(())
                // Error responses are logged where they are created, with their cause.
                .on_failure(()),
        )
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(CatchPanicLayer::new())
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(server.request_timeout_seconds),
        ))
        .layer(RequestBodyLimitLayer::new(server.max_body_bytes))
        .layer(cors(&server.cors_allowed_origins));

    router
        .route(
            "/openapi.json",
            get(move || async move { Json(openapi.as_ref().clone()) }),
        )
        .layer(middleware::from_fn_with_state(
            state.clone(),
            idempotency::middleware,
        ))
        .layer(cross_cutting)
        .with_state(state.clone())
}

fn cors(origins: &[String]) -> CorsLayer {
    let origins: Vec<HeaderValue> = origins
        .iter()
        .filter_map(|origin| origin.parse().ok())
        .collect();
    if origins.is_empty() {
        return CorsLayer::new();
    }
    CorsLayer::new()
        .allow_origin(origins)
        .allow_methods(tower_http::cors::Any)
        .allow_headers(tower_http::cors::Any)
}
