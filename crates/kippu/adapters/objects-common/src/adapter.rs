//! The bridge from `object_store` to Kippu's object storage port.

use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use futures_util::TryStreamExt;
use kippu_store::{BoxError, ObjectError, ObjectStorage, StoredObject};
use object_store::path::Path;
use object_store::{
    Attribute, Attributes, GetOptions, ObjectStore, ObjectStoreExt, PutOptions, PutPayload,
};

/// An [`ObjectStorage`] backed by any `object_store` implementation.
#[derive(Debug, Clone)]
pub struct ObjectStoreAdapter {
    store: Arc<dyn ObjectStore>,
}

impl ObjectStoreAdapter {
    /// Keeps objects in `store`.
    pub fn new(store: Arc<dyn ObjectStore>) -> Self {
        Self { store }
    }
}

fn failed(error: object_store::Error) -> ObjectError {
    match error {
        object_store::Error::NotFound { .. } => ObjectError::NotFound,
        other => ObjectError::failed(other),
    }
}

#[async_trait]
impl ObjectStorage for ObjectStoreAdapter {
    async fn put(
        &self,
        key: &str,
        bytes: Bytes,
        content_type: &str,
        cache_control: &str,
    ) -> Result<(), ObjectError> {
        let mut attributes = Attributes::new();
        attributes.insert(Attribute::ContentType, content_type.to_owned().into());
        attributes.insert(Attribute::CacheControl, cache_control.to_owned().into());
        let options = PutOptions {
            attributes,
            ..PutOptions::default()
        };
        self.store
            .put_opts(&Path::from(key), PutPayload::from_bytes(bytes), options)
            .await
            .map(drop)
            .map_err(failed)
    }

    async fn get(&self, key: &str) -> Result<StoredObject, ObjectError> {
        let object = self
            .store
            .get_opts(&Path::from(key), GetOptions::default())
            .await
            .map_err(failed)?;
        Ok(StoredObject {
            size: object.meta.size,
            body: Box::pin(
                object
                    .into_stream()
                    .map_err(|error| -> BoxError { Box::new(error) }),
            ),
        })
    }

    async fn delete(&self, key: &str) -> Result<(), ObjectError> {
        match self.store.delete(&Path::from(key)).await {
            Ok(()) | Err(object_store::Error::NotFound { .. }) => Ok(()),
            Err(error) => Err(ObjectError::failed(error)),
        }
    }
}
