//! Buying tickets under contention: purchase requests, reservations, and the workers that
//! process and expire them.

mod dto;
mod routes;
pub mod service;

use std::time::Duration;

use kippu_domain::account::Role;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::app::AppState;
use crate::auth::Permission;
use crate::config::Config;
use crate::module::{BackgroundTask, Module};

pub use dto::*;

/// Permissions checked by this module.
pub mod permissions {
    use crate::auth::Permission;

    /// Submit purchase requests and check out (account-scoped).
    pub const PURCHASES_CREATE: Permission = Permission::new("purchases.create");
    /// Read purchase requests and reservations (account-scoped: your own).
    pub const RESERVATIONS_READ: Permission = Permission::new("reservations.read");
}

/// The purchasing module.
#[derive(Debug, Clone, Copy, Default)]
pub struct Purchasing;

impl Module for Purchasing {
    fn name(&self) -> &'static str {
        "purchasing"
    }

    fn grants(&self) -> Vec<(Role, Permission)> {
        vec![
            (Role::User, permissions::PURCHASES_CREATE),
            (Role::User, permissions::RESERVATIONS_READ),
        ]
    }

    fn routes(&self) -> OpenApiRouter<AppState> {
        OpenApiRouter::new()
            .routes(routes!(routes::create_purchase_request))
            .routes(routes!(routes::get_purchase_request))
            .routes(routes!(routes::my_reservations))
            .routes(routes!(routes::get_reservation))
            .routes(routes!(routes::checkout_reservation))
            .routes(routes!(routes::cancel_reservation))
    }

    fn tasks(&self, config: &Config) -> Vec<BackgroundTask> {
        let workers = &config.workers;
        vec![
            BackgroundTask::every(
                "purchases",
                Duration::from_millis(workers.purchase_interval_ms),
                service::process_batch,
            ),
            BackgroundTask::every(
                "reservation-expiry",
                Duration::from_millis(workers.expiry_interval_ms),
                service::expire_batch,
            ),
        ]
    }
}
