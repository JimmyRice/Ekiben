//! Choosing a storage adapter from the database URL.

use std::sync::Arc;

use kippu_store::{BoxError, EventBus, ObjectStorage, PurchaseInbox, Store};

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
#[cfg_attr(
    not(feature = "nats"),
    expect(clippy::unused_async, reason = "only the NATS adapter awaits")
)]
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

/// The object storage adapters compiled into this binary, for error messages.
pub const COMPILED_OBJECT_STORAGE: &[&str] = &[
    #[cfg(feature = "s3")]
    "s3",
    #[cfg(feature = "gcs")]
    "gs",
    #[cfg(feature = "azure")]
    "az",
];

/// Connects to the object storage named by `url`, choosing the adapter by the URL's scheme.
/// `options` are the provider's settings (`images.options`).
pub fn connect_objects(
    url: &str,
    options: &[(String, String)],
) -> Result<Arc<dyn ObjectStorage>, BoxError> {
    let scheme = url.split_once(':').map_or(url, |(scheme, _)| scheme);
    match scheme {
        #[cfg(feature = "s3")]
        "s3" | "s3a" => Ok(Arc::new(kippu_objects_s3::connect(url, options)?)),
        #[cfg(feature = "gcs")]
        "gs" => Ok(Arc::new(kippu_objects_gcs::connect(url, options)?)),
        #[cfg(feature = "azure")]
        "az" | "azure" | "abfs" | "abfss" => {
            Ok(Arc::new(kippu_objects_azure::connect(url, options)?))
        }
        "file" | "memory" => Err("images.url must be an object store (s3://, gs://, az://): \
             instances are stateless, so images cannot live on one of them"
            .into()),
        _ => {
            let _ = options;
            Err(format!(
                "no object storage adapter for `{scheme}:` URLs; this binary supports: {}",
                COMPILED_OBJECT_STORAGE.join(", ")
            )
            .into())
        }
    }
}
