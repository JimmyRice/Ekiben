//! The waiting room: admission control in front of high-demand sales.
//!
//! ```text
//! join ──▶ queue ticket (position n) ──poll──▶ waiting … ──▶ admitted: admission pass ──▶ purchase
//!       admitted_through advances by a batch per tick once the sale opens, while the backlog is small
//! ```
//!
//! Positions come from the store; tickets and passes are signed tokens, so any instance can
//! serve any buyer without shared memory.

mod dto;
mod routes;
pub mod service;

use std::time::Duration;

use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::app::AppState;
use crate::config::Config;
use crate::module::{BackgroundTask, IdempotentRoute, Module};

pub use dto::*;

/// The admission module.
#[derive(Debug, Clone, Copy, Default)]
pub struct Admission;

impl Module for Admission {
    fn name(&self) -> &'static str {
        "admission"
    }

    fn routes(&self) -> OpenApiRouter<AppState> {
        OpenApiRouter::new()
            .routes(routes!(routes::get_waiting_room, routes::join_waiting_room))
            .routes(routes!(routes::request_admission))
    }

    fn idempotent_routes(&self) -> Vec<IdempotentRoute> {
        // An account keeps its position however often it joins.
        vec![IdempotentRoute::post("/v1/sales/{sale_id}/waiting-room")]
    }

    fn tasks(&self, config: &Config) -> Vec<BackgroundTask> {
        vec![BackgroundTask::every(
            "admission",
            Duration::from_millis(config.workers.admission_interval_ms.get()),
            service::admit_batches,
        )]
    }
}
