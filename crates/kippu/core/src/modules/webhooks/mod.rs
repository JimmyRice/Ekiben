//! Webhooks: integration events pushed to the integrator's endpoints as signed HTTP requests.
//!
//! Every change other systems may care about is written to the outbox in the same
//! transaction as the change itself (see [`kippu_domain::outbox`]). Instead of polling a feed,
//! an integrator can register a webhook: Kippu then POSTs each event to it, in order, at
//! least once, signed with the deployment's webhook key. Organizers register webhooks for
//! their organization and receive only its events; admins can register global ones.
//!
//! The wire protocol is specified in `spec/webhook-protocol.md`.

mod delivery;
mod dto;
mod routes;
pub mod service;
pub mod signature;

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

    /// Manage an organization's webhooks (organization-scoped).
    pub const WEBHOOKS_MANAGE: Permission = Permission::new("webhooks.manage");
    /// Manage webhooks that receive every event.
    pub const WEBHOOKS_MANAGE_ALL: Permission = Permission::new("webhooks.manage_all");
}

/// The webhooks module.
#[derive(Debug, Clone, Copy, Default)]
pub struct Webhooks;

impl Module for Webhooks {
    fn name(&self) -> &'static str {
        "webhooks"
    }

    fn grants(&self) -> Vec<(Role, Permission)> {
        vec![
            (Role::Organizer, permissions::WEBHOOKS_MANAGE),
            (Role::Admin, permissions::WEBHOOKS_MANAGE_ALL),
        ]
    }

    fn routes(&self) -> OpenApiRouter<AppState> {
        OpenApiRouter::new()
            .routes(routes!(
                routes::create_organization_webhook,
                routes::list_organization_webhooks
            ))
            .routes(routes!(
                routes::create_global_webhook,
                routes::list_global_webhooks
            ))
            .routes(routes!(
                routes::get_webhook,
                routes::patch_webhook,
                routes::delete_webhook
            ))
            .routes(routes!(routes::webhook_keys))
    }

    fn tasks(&self, config: &Config) -> Vec<BackgroundTask> {
        service::delivery_task(config).into_iter().collect()
    }
}
