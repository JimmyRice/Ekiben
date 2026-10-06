//! Connecting to PostgreSQL, and the store itself.

use std::str::FromStr;
use std::time::Duration;

use async_trait::async_trait;
use kippu_store::{Store, StoreCapabilities, StoreError, StoreResult, StoreTx};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

use crate::errors::error;
use crate::tx;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Connections per instance unless the URL says otherwise (`?max_connections=`).
const DEFAULT_MAX_CONNECTIONS: u32 = 16;

/// Kippu's storage on PostgreSQL.
///
/// Many transactions write at once. Correctness rests on row locks, never on serializing
/// writes in the application: conditional updates for stock and quotas, `SELECT … FOR UPDATE`
/// where a record is read and then changed, `FOR UPDATE SKIP LOCKED` to claim queued purchase
/// requests, and a transaction-scoped advisory lock that keeps outbox sequence numbers in
/// commit order.
#[derive(Debug, Clone)]
pub struct PostgresStore {
    pub(crate) pool: PgPool,
}

impl PostgresStore {
    /// Connects to `url`, e.g. `postgres://kippu:secret@db.internal/kippu`. Besides the usual
    /// libpq parameters, `max_connections` sets the pool size (default 16).
    pub async fn connect(url: &str) -> StoreResult<Self> {
        let (url, max_connections) = split_pool_size(url)?;
        let options = PgConnectOptions::from_str(&url).map_err(StoreError::backend)?;
        Self::connect_with(options, max_connections).await
    }

    /// Connects with explicit options, e.g. to set a `search_path` or TLS settings.
    pub async fn connect_with(
        options: PgConnectOptions,
        max_connections: u32,
    ) -> StoreResult<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(max_connections)
            .acquire_timeout(Duration::from_secs(30))
            .connect_with(options)
            .await
            .map_err(error)?;
        Ok(Self { pool })
    }

    /// The connection pool. Extension modules may use it for their own tables.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}

/// Removes Kippu's own `max_connections` parameter, which libpq would reject.
fn split_pool_size(url: &str) -> StoreResult<(String, u32)> {
    let Some((base, query)) = url.split_once('?') else {
        return Ok((url.to_owned(), DEFAULT_MAX_CONNECTIONS));
    };
    let mut max_connections = DEFAULT_MAX_CONNECTIONS;
    let mut kept = Vec::new();
    for pair in query.split('&') {
        match pair.strip_prefix("max_connections=") {
            Some(value) => {
                max_connections = value
                    .parse()
                    .map_err(|_| StoreError::backend("max_connections must be a number"))?;
            }
            None => kept.push(pair),
        }
    }
    let url = if kept.is_empty() {
        base.to_owned()
    } else {
        format!("{base}?{}", kept.join("&"))
    };
    Ok((url, max_connections))
}

#[async_trait]
impl Store for PostgresStore {
    fn capabilities(&self) -> StoreCapabilities {
        StoreCapabilities {
            backend: "postgres",
            concurrent_writers: true,
        }
    }

    async fn migrate(&self) -> StoreResult<()> {
        MIGRATOR.run(&self.pool).await.map_err(StoreError::backend)
    }

    async fn ping(&self) -> StoreResult<()> {
        sqlx::query("SELECT 1")
            .execute(&self.pool)
            .await
            .map_err(error)?;
        Ok(())
    }

    async fn begin(&self) -> StoreResult<Box<dyn StoreTx>> {
        let tx = self.pool.begin().await.map_err(error)?;
        Ok(Box::new(tx::PostgresTx::new(tx)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pool_size_parameter_is_ours() {
        assert_eq!(
            split_pool_size("postgres://db/kippu").unwrap(),
            ("postgres://db/kippu".to_owned(), DEFAULT_MAX_CONNECTIONS)
        );
        assert_eq!(
            split_pool_size("postgres://db/kippu?max_connections=64&sslmode=require").unwrap(),
            ("postgres://db/kippu?sslmode=require".to_owned(), 64)
        );
        assert!(split_pool_size("postgres://db/kippu?max_connections=many").is_err());
    }
}
