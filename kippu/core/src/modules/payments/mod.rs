//! Payment attestation: how Kippu learns money really arrived, without knowing any payment
//! provider.
//!
//! A deployment registers *attestors* — its own small services wrapping Stripe, Alipay, a
//! cash desk, anything — each with Ed25519 keys. An attestor learns what to charge from
//! `payment.requested` events (or `GET /v1/attestor/reservations/{id}`), charges the buyer
//! however it likes, and reports the result with a signed `POST /v1/payment-attestations`.
//! Payment methods are added or removed by registering or revoking attestors at runtime.
//!
//! The wire protocol is specified in `spec/attestor-protocol.md`.

mod dto;
mod routes;
pub(crate) mod service;
pub mod signature;

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

    /// Register, revoke and re-key attestors.
    pub const ATTESTORS_MANAGE: Permission = Permission::new("attestors.manage");
    /// Record payments taken in person (organization-scoped).
    pub const PAYMENTS_MANUAL: Permission = Permission::new("payments.manual");
    /// Read the full integration event feed.
    pub const FEED_READ: Permission = Permission::new("feed.read");
}

/// The payments module.
#[derive(Debug, Clone, Copy, Default)]
pub struct Payments;

impl Module for Payments {
    fn name(&self) -> &'static str {
        "payments"
    }

    fn grants(&self) -> Vec<(Role, Permission)> {
        vec![
            (Role::Admin, permissions::ATTESTORS_MANAGE),
            (Role::Admin, permissions::FEED_READ),
            (Role::Organizer, permissions::PAYMENTS_MANUAL),
        ]
    }

    fn routes(&self) -> OpenApiRouter<AppState> {
        OpenApiRouter::new()
            .routes(routes!(routes::create_attestor, routes::list_attestors))
            .routes(routes!(routes::set_attestor_revoked))
            .routes(routes!(routes::add_attestor_key))
            .routes(routes!(routes::set_attestor_key_revoked))
            .routes(routes!(routes::attestor_reservation))
            .routes(routes!(routes::record_attestation))
            .routes(routes!(routes::manual_payment))
            .routes(routes!(routes::attestor_feed))
            .routes(routes!(routes::admin_feed))
    }
}
