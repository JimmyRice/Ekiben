//! Liveness and readiness probes.

use axum::extract::State;

use crate::app::AppState;
use crate::error::{ApiError, Problem};

/// Liveness: the process is up.
#[utoipa::path(get, path = "/healthz", tag = "health", responses((status = 200, description = "Alive")))]
pub(crate) async fn healthz() -> &'static str {
    "ok"
}

/// Readiness: the database is reachable.
#[utoipa::path(
    get,
    path = "/readyz",
    tag = "health",
    responses((status = 200, description = "Ready"), (status = 503, body = Problem))
)]
pub(crate) async fn readyz(state: State<AppState>) -> Result<&'static str, ApiError> {
    state.store().ping().await?;
    Ok("ready")
}
