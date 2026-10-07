/// A boxed error from a storage backend.
pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Why a storage operation failed.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// A uniqueness or version check failed. The string names what conflicted, e.g. `"email"`.
    #[error("conflict on {0}")]
    Conflict(&'static str),
    /// The record an operation needs does not exist.
    #[error("{0} not found")]
    NotFound(&'static str),
    /// The backend is temporarily unreachable or overloaded. Retrying later may succeed.
    #[error("storage temporarily unavailable")]
    Unavailable(#[source] BoxError),
    /// Anything else: a bug, corrupt data or a misconfigured database.
    #[error("storage failure")]
    Backend(#[source] BoxError),
}

impl StoreError {
    /// Wraps an unexpected backend error.
    pub fn backend(error: impl Into<BoxError>) -> Self {
        Self::Backend(error.into())
    }
}

/// Result of a storage operation.
pub type StoreResult<T> = Result<T, StoreError>;
