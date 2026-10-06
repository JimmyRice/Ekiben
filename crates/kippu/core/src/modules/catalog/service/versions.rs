//! Optimistic concurrency for catalog edits.

use crate::error::{ApiError, ApiResult};

/// Fails with 412 unless the caller edits the version that is stored. Changes are applied on
/// top of the stored record, so they must not be merged onto a version the caller never saw.
pub(super) fn check_version(stored: i64, expected: i64) -> ApiResult<()> {
    if stored == expected {
        Ok(())
    } else {
        Err(ApiError::stale_version())
    }
}
