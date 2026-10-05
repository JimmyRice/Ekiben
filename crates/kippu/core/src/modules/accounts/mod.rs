//! Accounts, sessions, organizations and the audit log.
//!
//! - Anyone may register a `user` account and sign in.
//! - Admins (and root) create admin and organizer accounts, organizations and memberships.
//! - Root signs in with a key-signed token instead (see [`crate::auth::tokens`]).
//! - Accounts may also sign in through external providers (see [`crate::auth::external`]);
//!   their owners set an email and password, and unlink providers, under `/v1/me`.

mod dto;
mod routes;
mod service;

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

    /// Create and delete accounts of lower roles.
    pub const ACCOUNTS_MANAGE: Permission = Permission::new("accounts.manage");
    /// Create organizations and manage their members.
    pub const ORGANIZATIONS_MANAGE: Permission = Permission::new("organizations.manage");
    /// Read the audit log.
    pub const AUDIT_READ: Permission = Permission::new("audit.read");
    /// Manage your own sign-in methods: email, password, external identities
    /// (account-scoped).
    pub const PROFILE_MANAGE: Permission = Permission::new("profile.manage");
}

/// The accounts module.
#[derive(Debug, Clone, Copy, Default)]
pub struct Accounts;

impl Module for Accounts {
    fn name(&self) -> &'static str {
        "accounts"
    }

    fn grants(&self) -> Vec<(Role, Permission)> {
        vec![
            (Role::Admin, permissions::ACCOUNTS_MANAGE),
            (Role::Admin, permissions::ORGANIZATIONS_MANAGE),
            (Role::Admin, permissions::AUDIT_READ),
            (Role::User, permissions::PROFILE_MANAGE),
        ]
    }

    fn routes(&self) -> OpenApiRouter<AppState> {
        OpenApiRouter::new()
            .routes(routes!(routes::register))
            .routes(routes!(routes::login))
            .routes(routes!(routes::refresh))
            .routes(routes!(routes::logout))
            .routes(routes!(routes::me))
            .routes(routes!(routes::set_email))
            .routes(routes!(routes::change_password))
            .routes(routes!(routes::list_identities))
            .routes(routes!(routes::unlink_identity))
            .routes(routes!(routes::create_account, routes::list_accounts))
            .routes(routes!(routes::delete_account))
            .routes(routes!(
                routes::create_organization,
                routes::list_organizations
            ))
            .routes(routes!(routes::add_member, routes::remove_member))
            .routes(routes!(routes::audit_log))
    }
}
