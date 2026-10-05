//! Choosing a storage adapter from the database URL.

use std::sync::Arc;

use kippu_store::{BoxError, Store};

/// The adapters compiled into this binary, for error messages.
pub const COMPILED_ADAPTERS: &[&str] = &[
    #[cfg(feature = "sqlite")]
    "sqlite",
    #[cfg(feature = "postgres")]
    "postgres",
    #[cfg(feature = "mysql")]
    "mysql",
];

/// Connects to the database named by `url`, choosing the adapter by the URL's scheme.
pub async fn connect(url: &str) -> Result<Arc<dyn Store>, BoxError> {
    let scheme = url.split_once(':').map_or(url, |(scheme, _)| scheme);
    match scheme {
        #[cfg(feature = "sqlite")]
        "sqlite" => Ok(Arc::new(
            kippu_store_sqlite::SqliteStore::connect(url).await?,
        )),
        #[cfg(feature = "postgres")]
        "postgres" | "postgresql" => Ok(Arc::new(
            kippu_store_postgres::PostgresStore::connect(url).await?,
        )),
        #[cfg(feature = "mysql")]
        "mysql" => Ok(Arc::new(kippu_store_mysql::MySqlStore::connect(url).await?)),
        _ => Err(format!(
            "no adapter for `{scheme}:` URLs; this binary supports: {}",
            COMPILED_ADAPTERS.join(", ")
        )
        .into()),
    }
}
