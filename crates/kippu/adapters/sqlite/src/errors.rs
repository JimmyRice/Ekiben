//! Turning sqlx errors into the store's errors.

use kippu_store::StoreError;

/// Maps an sqlx error to a [`StoreError`]. Constraint violations become conflicts named after
/// the kind of constraint; callers that know which column was involved rename them.
pub(crate) fn error(error: sqlx::Error) -> StoreError {
    use sqlx::error::ErrorKind;

    match &error {
        sqlx::Error::Database(database) => match database.kind() {
            ErrorKind::UniqueViolation => StoreError::Conflict("unique"),
            ErrorKind::ForeignKeyViolation => StoreError::Conflict("reference"),
            ErrorKind::CheckViolation => StoreError::Conflict("constraint"),
            // SQLITE_BUSY and SQLITE_LOCKED: another connection holds the lock.
            _ if matches!(database.code().as_deref(), Some("5" | "6")) => {
                StoreError::Unavailable(Box::new(error))
            }
            _ => StoreError::backend(error),
        },
        sqlx::Error::PoolTimedOut | sqlx::Error::PoolClosed | sqlx::Error::Io(_) => {
            StoreError::Unavailable(Box::new(error))
        }
        _ => StoreError::backend(error),
    }
}

/// Like [`error`], but reports a unique violation as a conflict on `field`.
pub(crate) fn unique(field: &'static str) -> impl Fn(sqlx::Error) -> StoreError {
    move |error| match self::error(error) {
        StoreError::Conflict("unique") => StoreError::Conflict(field),
        other => other,
    }
}
