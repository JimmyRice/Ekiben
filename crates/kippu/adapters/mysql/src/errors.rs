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
            // Lock wait timeout, deadlock, and a cached statement invalidated by the server
            // (1615, e.g. when its table definition cache overflows): safe to retry.
            _ if matches!(database.code().as_deref(), Some("1205" | "1213" | "40001"))
                || matches!(
                    database.try_downcast_ref::<sqlx::mysql::MySqlDatabaseError>(),
                    Some(mysql) if matches!(mysql.number(), 1205 | 1213 | 1615)
                ) =>
            {
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

/// Like [`error`], naming the unique constraint `field`.
pub(crate) fn unique(field: &'static str) -> impl Fn(sqlx::Error) -> StoreError {
    move |error| match self::error(error) {
        StoreError::Conflict("unique") => StoreError::Conflict(field),
        other => other,
    }
}

/// Whether an error is a duplicate key: MySQL has no `ON CONFLICT DO NOTHING` that leaves
/// other errors alone, so inserts that may collide run plainly and catch this.
pub(crate) fn is_duplicate(error: &sqlx::Error) -> bool {
    matches!(error, sqlx::Error::Database(database)
        if database.kind() == sqlx::error::ErrorKind::UniqueViolation)
}
