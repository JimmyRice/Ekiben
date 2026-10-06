//! Where images are kept.

use std::sync::Arc;

use kippu_store::ObjectStorage;

/// Where images are kept, and how clients reach them.
#[derive(Debug, Clone)]
pub struct ImageStorage {
    /// The object storage adapter, already scoped to the configured bucket and prefix.
    pub objects: Arc<dyn ObjectStorage>,
    /// Images are served by redirecting here, if set.
    pub public_base_url: Option<String>,
}
