//! Catalog use cases: who may see and edit events, sales and ticket types, and what editing
//! them means. Independent of HTTP — `routes.rs` only translates requests into these calls.
//!
//! Other modules use the visibility checks here ([`visible_event`], [`writable_sale`], …) so
//! that "who may see a draft" is decided in one place.

mod events;
mod favorites;
mod sales;
mod ticket_types;

pub use events::*;
pub use favorites::*;
pub use sales::*;
pub use ticket_types::*;

use crate::error::{ApiError, ApiResult};

/// Fails with 412 unless the caller edits the version that is stored. Changes are applied on
/// top of the stored record, so they must not be merged onto a version the caller never saw.
fn check_version(stored: i64, expected: i64) -> ApiResult<()> {
    if stored == expected {
        Ok(())
    } else {
        Err(ApiError::stale_version())
    }
}
