//! What is on sale: events, their sales and ticket types, and buyers' favourites.
//!
//! Anyone may browse published events. Organizers create and edit events of their own
//! organizations; drafts are visible only to them.

mod dto;
mod routes;
pub(crate) mod service;

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

    /// Create and edit events, sales and ticket types (organization-scoped); see drafts.
    pub const EVENTS_WRITE: Permission = Permission::new("events.write");
    /// Keep a list of favourite events (account-scoped).
    pub const FAVORITES_MANAGE: Permission = Permission::new("favorites.manage");
}

/// The catalog module.
#[derive(Debug, Clone, Copy, Default)]
pub struct Catalog;

impl Module for Catalog {
    fn name(&self) -> &'static str {
        "catalog"
    }

    fn grants(&self) -> Vec<(Role, Permission)> {
        vec![
            (Role::Organizer, permissions::EVENTS_WRITE),
            (Role::User, permissions::FAVORITES_MANAGE),
        ]
    }

    fn routes(&self) -> OpenApiRouter<AppState> {
        OpenApiRouter::new()
            .routes(routes!(routes::list_events))
            .routes(routes!(
                routes::create_event,
                routes::list_organization_events
            ))
            .routes(routes!(
                routes::get_event,
                routes::update_event,
                routes::patch_event
            ))
            .routes(routes!(routes::create_sale, routes::list_sales))
            .routes(routes!(
                routes::get_sale,
                routes::update_sale,
                routes::patch_sale
            ))
            .routes(routes!(routes::create_ticket_type))
            .routes(routes!(
                routes::update_ticket_type,
                routes::patch_ticket_type
            ))
            .routes(routes!(routes::list_favorites))
            .routes(routes!(routes::add_favorite, routes::remove_favorite))
    }
}
