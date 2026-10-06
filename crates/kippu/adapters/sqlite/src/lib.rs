#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

mod accounts;
mod catalog;
mod convert;
mod housekeeping;
mod images;
mod outbox;
mod payments;
mod purchasing;
mod ticketing;
mod tx;
mod webhooks;

use std::str::FromStr;
use std::time::Duration;

use async_trait::async_trait;
use kippu_store::{Store, StoreCapabilities, StoreError, StoreResult, StoreTx};
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// How many connections serve reads when the database is a file.
const READ_CONNECTIONS: u32 = 8;

/// Kippu's storage on SQLite.
///
/// SQLite allows one writer at a time, so every write — transactional or not — goes through a
/// single-connection *writer* pool, which serializes them in the application instead of
/// letting them collide on `SQLITE_BUSY`. In WAL mode readers never block the writer, so reads
/// use a separate pool. An in-memory database has one connection for everything.
#[derive(Debug, Clone)]
pub struct SqliteStore {
    writer: SqlitePool,
    reader: SqlitePool,
}

impl SqliteStore {
    /// Opens (creating if needed) the database at `url`, e.g. `sqlite:///var/lib/kippu.db`
    /// or `sqlite::memory:`.
    pub async fn connect(url: &str) -> StoreResult<Self> {
        let options = SqliteConnectOptions::from_str(url)
            .map_err(StoreError::backend)?
            .create_if_missing(true)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(5));

        if url.contains(":memory:") || url.contains("mode=memory") {
            let pool = pool(1).connect_with(options).await.map_err(error)?;
            return Ok(Self {
                writer: pool.clone(),
                reader: pool,
            });
        }

        let options = options
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal);
        let writer = pool(1).connect_with(options.clone()).await.map_err(error)?;
        let reader = pool(READ_CONNECTIONS)
            .connect_with(options.read_only(true))
            .await
            .map_err(error)?;
        Ok(Self { writer, reader })
    }

    /// The pool that serializes writes. Extension modules may use it for their own tables.
    pub fn writer(&self) -> &SqlitePool {
        &self.writer
    }

    /// The pool for reads. Extension modules may use it for their own tables.
    pub fn reader(&self) -> &SqlitePool {
        &self.reader
    }
}

fn pool(max_connections: u32) -> SqlitePoolOptions {
    SqlitePoolOptions::new()
        .max_connections(max_connections)
        .acquire_timeout(Duration::from_secs(30))
}

#[async_trait]
impl Store for SqliteStore {
    fn capabilities(&self) -> StoreCapabilities {
        StoreCapabilities {
            backend: "sqlite",
            concurrent_writers: false,
        }
    }

    async fn migrate(&self) -> StoreResult<()> {
        MIGRATOR
            .run(&self.writer)
            .await
            .map_err(StoreError::backend)
    }

    async fn ping(&self) -> StoreResult<()> {
        sqlx::query("SELECT 1")
            .execute(&self.reader)
            .await
            .map_err(error)?;
        Ok(())
    }

    async fn begin(&self) -> StoreResult<Box<dyn StoreTx>> {
        let tx = self
            .writer
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(error)?;
        Ok(Box::new(tx::SqliteTx::new(tx)))
    }
}

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
