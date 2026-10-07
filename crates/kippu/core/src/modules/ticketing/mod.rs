//! Issued tickets, and the public keys gates use to verify them.

mod dto;
mod routes;
pub mod service;

use kippu_domain::account::Role;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::app::AppState;
use crate::auth::Permission;
use crate::module::Module;

pub use dto::*;

/// Permissions checked by this module.
pub mod permissions {
    use crate::auth::Permission;

    /// Read tickets (account-scoped: your own).
    pub const TICKETS_READ: Permission = Permission::new("tickets.read");
}

/// The ticketing module.
#[derive(Debug, Clone, Copy, Default)]
pub struct Ticketing;

impl Module for Ticketing {
    fn name(&self) -> &'static str {
        "ticketing"
    }

    fn grants(&self) -> Vec<(Role, Permission)> {
        vec![(Role::User, permissions::TICKETS_READ)]
    }

    fn routes(&self) -> OpenApiRouter<AppState> {
        OpenApiRouter::new()
            .routes(routes!(routes::ticket_keys))
            .routes(routes!(routes::my_tickets))
            .routes(routes!(routes::get_ticket))
            .routes(routes!(routes::get_ticket_raw))
    }
}
