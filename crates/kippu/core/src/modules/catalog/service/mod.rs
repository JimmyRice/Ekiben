//! Catalog use cases: who may see organizations, and who may see and edit events, sales,
//! their invitation lists and ticket types, and what editing them means. Independent of
//! HTTP — `routes.rs` only translates requests into these calls.
//!
//! Other modules use the visibility checks here ([`visible_event`], [`visible_sale`],
//! [`writable_sale`], …) so that who may see a draft, or a sale that is not public, is
//! decided in one place.

mod events;
mod favorites;
mod invitations;
mod organizations;
mod sales;
mod ticket_types;
mod versions;

pub use events::*;
pub use favorites::*;
pub use invitations::*;
pub use organizations::*;
pub use sales::*;
pub use ticket_types::*;

use versions::check_version;
