//! Catalog use cases: who may see and edit events, sales and ticket types, and what editing
//! them means. Independent of HTTP — `routes.rs` only translates requests into these calls.
//!
//! Other modules use the visibility checks here ([`visible_event`], [`writable_sale`], …) so
//! that "who may see a draft" is decided in one place.

mod events;
mod favorites;
mod sales;
mod ticket_types;
mod versions;

pub use events::*;
pub use favorites::*;
pub use sales::*;
pub use ticket_types::*;

use versions::check_version;
