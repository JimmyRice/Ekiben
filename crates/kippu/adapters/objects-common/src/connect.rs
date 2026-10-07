//! Turning an `images.url` and `images.options` into a connected store.

use std::str::FromStr;
use std::sync::Arc;

use kippu_store::BoxError;
use object_store::ObjectStore;
use object_store::prefix::PrefixStore;

use crate::ObjectStoreAdapter;

/// A provider's settings, as the caller configured them: names as the provider's
/// environment variables in lower case (`aws_endpoint`, …), with their values.
#[derive(Debug, Clone, Copy)]
pub struct Settings<'a>(pub &'a [(String, String)]);

impl Settings<'_> {
    /// The settings as the provider's own config keys, rejecting names it does not know.
    pub fn parse<K: FromStr>(&self) -> Result<Vec<(K, String)>, BoxError> {
        self.0
            .iter()
            .map(|(name, value)| {
                let key = name
                    .parse::<K>()
                    .map_err(|_| format!("images.options.{name} is not a setting of this store"))?;
                Ok((key, value.clone()))
            })
            .collect()
    }
}

/// Connects to the store at `url`.
///
/// `build` creates the provider's store from the URL (applying the options); the path of the
/// URL becomes a key prefix, so several deployments can share a bucket.
pub fn connect<S: ObjectStore>(
    url: &str,
    build: impl FnOnce(&str) -> Result<S, BoxError>,
) -> Result<ObjectStoreAdapter, BoxError> {
    let parsed =
        reqwest::Url::parse(url).map_err(|error| format!("images.url is not a URL: {error}"))?;
    // object_store builds its HTTP client with the process-wide rustls provider.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let store: Arc<dyn ObjectStore> = Arc::new(build(url)?);
    let prefix = parsed.path().trim_matches('/');
    let store = if prefix.is_empty() {
        store
    } else {
        Arc::new(PrefixStore::new(store, prefix))
    };
    Ok(ObjectStoreAdapter::new(store))
}
