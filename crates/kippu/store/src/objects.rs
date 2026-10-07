//! The object storage port: where event images (and anything else too big for the database)
//! are kept. Instances are stateless, so this is always a shared service — never a local
//! disk.

use std::fmt;

use async_trait::async_trait;
use bytes::Bytes;
use futures_util::stream::BoxStream;

use crate::BoxError;

/// Why an object storage call failed.
#[derive(Debug, thiserror::Error)]
pub enum ObjectError {
    /// There is no object under that key.
    #[error("no such object")]
    NotFound,
    /// The service failed or could not be reached. Safe to retry.
    #[error("object storage failed")]
    Failed(#[source] BoxError),
}

impl ObjectError {
    /// A failure of the service itself.
    pub fn failed(error: impl Into<BoxError>) -> Self {
        Self::Failed(error.into())
    }
}

/// An object being read: its size, and its bytes as they arrive.
pub struct StoredObject {
    /// The object's size in bytes.
    pub size: u64,
    /// The object's bytes, in order.
    pub body: BoxStream<'static, Result<Bytes, BoxError>>,
}

impl fmt::Debug for StoredObject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StoredObject")
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

/// A bucket-like service that keeps immutable blobs under string keys.
///
/// Keys are `/`-separated and never start with `/`; the adapter scopes them to whatever
/// bucket and prefix it was configured with. Objects are small enough to hold in memory
/// (images are capped by `images.max_bytes`), so writes take the whole payload; reads stream.
///
/// **Contract** (checked by [`object_conformance_tests!`](crate::object_conformance_tests)):
/// once `put` returns, `get` returns exactly those bytes; `put` to an existing key replaces
/// it; `get` of an absent key is [`ObjectError::NotFound`]; `delete` of an absent key
/// succeeds, so a retried delete is harmless.
#[async_trait]
pub trait ObjectStorage: fmt::Debug + Send + Sync {
    /// Stores `bytes` under `key`, with the media type and `Cache-Control` value the service
    /// should serve them with if it serves them directly.
    async fn put(
        &self,
        key: &str,
        bytes: Bytes,
        content_type: &str,
        cache_control: &str,
    ) -> Result<(), ObjectError>;

    /// Opens the object under `key`.
    async fn get(&self, key: &str) -> Result<StoredObject, ObjectError>;

    /// Removes the object under `key`, if there is one.
    async fn delete(&self, key: &str) -> Result<(), ObjectError>;
}
