//! Putting the API together: module routes, health checks, OpenAPI and cross-cutting
//! middleware.

use std::sync::Arc;
use std::time::Duration;

use axum::http::{HeaderValue, StatusCode};
use axum::routing::get;
use axum::{Router, middleware};
use tower::ServiceBuilder;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::cors::CorsLayer;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;
use utoipa::OpenApi;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::body_limit::{BodyLimits, limit_body};
use super::openapi::ApiDoc;
use super::{Json, health, idempotency, problems, trace};
use crate::app::AppState;
use crate::module::Module;

pub(crate) fn router(state: &AppState, modules: &[Arc<dyn Module>]) -> Router {
    let api = modules.iter().fold(
        OpenApiRouter::with_openapi(ApiDoc::openapi())
            .routes(routes!(health::healthz))
            .routes(routes!(health::readyz)),
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
        .layer(middleware::from_fn(problems::framework_errors))
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(CatchPanicLayer::new())
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(server.request_timeout_seconds),
        ))
        .layer(middleware::from_fn_with_state(
            Arc::new(BodyLimits {
                default: server.max_body_bytes,
                routes: modules
                    .iter()
                    .flat_map(|module| module.body_limits(state.config()))
                    .collect(),
            }),
            limit_body,
        ))
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
