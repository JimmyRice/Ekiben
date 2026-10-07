//! Denials: deny lists of tickets and accounts refused entry, and what gates must refuse.
//!
//! Organizers keep a list per event and one for every event of their organization. An entry
//! names a ticket or an account; a denied account cannot buy tickets for the events its denial
//! covers (its purchase requests are rejected with `account_denied`).
//!
//! Gates verify tickets offline and never learn who holds a ticket, so what they need is a set
//! of ticket ids: `GET /v1/events/{id}/denied-tickets` joins the tickets denied directly, the
//! tickets of denied accounts and the tickets revoked by refunds. A gate checks each verified
//! ticket's `ticket_id` against it.
//!
//! [`service`] holds the use cases, `routes.rs` the HTTP handlers and `dto.rs` their bodies.

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

    /// Add, change and lift denials (organization-scoped).
    pub const DENIALS_MANAGE: Permission = Permission::new("denials.manage");
    /// Read denials and the tickets gates must refuse (organization-scoped).
    pub const DENIALS_READ: Permission = Permission::new("denials.read");
}

/// The denials module.
#[derive(Debug, Clone, Copy, Default)]
pub struct Denials;

impl Module for Denials {
    fn name(&self) -> &'static str {
        "denials"
    }

    fn grants(&self) -> Vec<(Role, Permission)> {
        vec![
            (Role::Organizer, permissions::DENIALS_MANAGE),
            (Role::Organizer, permissions::DENIALS_READ),
        ]
    }

    fn routes(&self) -> OpenApiRouter<AppState> {
        OpenApiRouter::new()
            .routes(routes!(
                routes::create_organization_denial,
                routes::list_organization_denials
            ))
            .routes(routes!(
                routes::create_event_denial,
                routes::list_event_denials
            ))
            .routes(routes!(
                routes::get_denial,
                routes::patch_denial,
                routes::delete_denial
            ))
            .routes(routes!(routes::denied_tickets))
    }
}
