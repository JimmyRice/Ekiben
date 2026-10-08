//! Payment attestation: how Kippu learns money really arrived, without knowing any payment
//! provider.
//!
//! A deployment registers *attestors* — its own small services wrapping Stripe, Alipay, a
//! cash desk, anything — each with Ed25519 keys. An attestor learns what to charge from
//! `payment.requested` events (or `GET /v1/attestor/reservations/{id}`), charges the buyer
//! however it likes, and reports the result with a signed `POST /v1/payment-attestations`.
//! Payment methods are added or removed by registering or revoking attestors at runtime.
//!
//! Refunds of issued tickets are asked of Kippu (`POST /v1/reservations/{id}/refunds`), never
//! of an attestor: the tickets are revoked at once and the attestor is asked to return the
//! money (`refund.required` with a `refund_id`), which it confirms. An attestor reports money
//! that went back on its own (a chargeback) as `reversed`, which revokes the tickets.
//!
//! The wire protocol is specified in `spec/attestor-protocol.md`.

mod dto;
mod routes;
pub mod service;
pub mod signature;

use kippu_domain::account::Role;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::app::AppState;
use crate::auth::Permission;
use crate::module::{IdempotentRoute, Module};

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
    /// Refund one's own tickets within their refund period, and read those refunds
    /// (account-scoped).
    pub const REFUNDS_REQUEST: Permission = Permission::new("refunds.request");
    /// Refund any ticket of the organization's events at any time, and read the refunds
    /// (organization-scoped).
    pub const REFUNDS_MANAGE: Permission = Permission::new("refunds.manage");
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
            (Role::User, permissions::REFUNDS_REQUEST),
            (Role::Organizer, permissions::REFUNDS_MANAGE),
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
            .routes(routes!(routes::refund_tickets, routes::reservation_refunds))
            .routes(routes!(routes::get_refund))
            .routes(routes!(routes::confirm_manual_refund))
    }

    fn idempotent_routes(&self) -> Vec<IdempotentRoute> {
        // Attestations are unique per attestor; confirming a refund twice changes nothing.
        vec![
            IdempotentRoute::post("/v1/payment-attestations"),
            IdempotentRoute::post("/v1/reservations/{reservation_id}/manual-payment"),
            IdempotentRoute::post("/v1/refunds/{refund_id}/manual-confirmation"),
        ]
    }
}
