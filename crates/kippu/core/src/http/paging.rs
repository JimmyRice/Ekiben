//! Pagination parameters shared by list endpoints.

use serde::Deserialize;
use utoipa::IntoParams;

/// Keyset pagination parameters shared by every list endpoint.
#[derive(Debug, Deserialize, IntoParams)]
pub struct PageQuery {
    /// At most this many items (default 50, at most 200).
    pub limit: Option<u32>,
    /// Return items whose id sorts after this one.
    pub after: Option<uuid::Uuid>,
}

impl PageQuery {
    /// The page to fetch from the store.
    pub fn page(&self) -> kippu_store::PageRequest {
        kippu_store::PageRequest {
            limit: self.limit.unwrap_or(50).clamp(1, 200),
            after: self.after,
        }
    }
}
