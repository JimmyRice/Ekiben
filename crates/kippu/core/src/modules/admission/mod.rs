//! The waiting room: admission control in front of high-demand sales.
//!
//! ```text
//! join ──▶ queue ticket (position n) ──poll──▶ waiting … ──▶ admitted: admission pass ──▶ purchase
//!                         admitted_through advances by a batch per tick, while the backlog is small
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
use crate::module::{BackgroundTask, Module};

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
            .routes(routes!(routes::join_waiting_room))
            .routes(routes!(routes::request_admission))
    }

    fn tasks(&self, config: &Config) -> Vec<BackgroundTask> {
        vec![BackgroundTask::every(
            "admission",
            Duration::from_millis(config.workers.admission_interval_ms.get()),
            service::admit_batches,
        )]
    }
}
