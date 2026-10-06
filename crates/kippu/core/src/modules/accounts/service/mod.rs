//! Account use cases, independent of HTTP: signing up and in, managing your own sign-in
//! methods, and administering accounts, organizations and the audit log.
//!
//! Sessions themselves (issuing, refreshing, revoking tokens) are
//! [`crate::auth::sessions`]; external sign-ins are [`crate::auth::external`].

mod admin;
mod new_account;
mod profile;
mod sign_in;

pub use admin::*;
pub use new_account::NewAccount;
pub use profile::*;
pub use sign_in::*;

use new_account::insert_account;
