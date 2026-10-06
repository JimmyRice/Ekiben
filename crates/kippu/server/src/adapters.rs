//! Choosing a storage adapter from the database URL.

use std::sync::Arc;

use kippu_store::{BoxError, EventBus, PurchaseInbox, Store};

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

/// The queue adapters compiled into this binary, for error messages.
pub const COMPILED_QUEUES: &[&str] = &[
    #[cfg(feature = "nats")]
    "nats",
];

/// A connected message queue: the purchase inbox and the event bus it provides.
pub struct Queue {
    /// Where purchase requests wait before the database.
    pub inbox: Arc<dyn PurchaseInbox>,
    /// Where integration events are published.
    pub bus: Arc<dyn EventBus>,
}

/// Connects to the queue named by `url`, choosing the adapter by the URL's scheme.
pub async fn connect_queue(url: &str) -> Result<Queue, BoxError> {
    let scheme = url.split_once(':').map_or(url, |(scheme, _)| scheme);
    match scheme {
        #[cfg(feature = "nats")]
        "nats" | "tls" => {
            let queue = Arc::new(
                kippu_nats::NatsQueue::connect(url, kippu_nats::NatsOptions::default()).await?,
            );
            Ok(Queue {
                inbox: queue.clone(),
                bus: queue,
            })
        }
        _ => Err(format!(
            "no queue adapter for `{scheme}:` URLs; this binary supports: {}",
            COMPILED_QUEUES.join(", ")
        )
        .into()),
    }
}
