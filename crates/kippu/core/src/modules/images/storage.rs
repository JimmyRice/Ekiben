//! Connecting to the object store images are kept in.

use std::sync::Arc;

use object_store::ObjectStore;
use object_store::aws::{AmazonS3Builder, AmazonS3ConfigKey};
use object_store::azure::{AzureConfigKey, MicrosoftAzureBuilder};
use object_store::gcp::{GoogleCloudStorageBuilder, GoogleConfigKey};
use object_store::prefix::PrefixStore;
use secrecy::ExposeSecret;

use crate::config::ImagesConfig;

/// Where images are kept, and how clients reach them.
#[derive(Debug, Clone)]
pub struct ImageStorage {
    /// The object store, already scoped to the configured prefix.
    pub store: Arc<dyn ObjectStore>,
    /// Images are served by redirecting here, if set.
    pub public_base_url: Option<String>,
}

fn configured<K: std::str::FromStr>(
    options: &std::collections::BTreeMap<String, secrecy::SecretString>,
) -> Result<Vec<(K, String)>, String> {
    options
        .iter()
        .map(|(name, value)| {
            let key = name
                .parse::<K>()
                .map_err(|_| format!("images.options.{name} is not a setting of this store"))?;
            Ok((key, value.expose_secret().to_owned()))
        })
        .collect()
}

/// Connects to the store named by `images.url`, or `None` if images are not configured.
///
/// Only object stores are accepted: a local directory or memory would tie images to one
/// instance, and Kippu's instances are interchangeable.
pub(crate) fn connect(config: &ImagesConfig) -> Result<Option<ImageStorage>, String> {
    let Some(url) = &config.url else {
        return Ok(None);
    };
    let parsed =
        reqwest::Url::parse(url).map_err(|error| format!("images.url is not a URL: {error}"))?;
    // object_store builds its HTTP client with the process-wide rustls provider.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let store: Arc<dyn ObjectStore> = match parsed.scheme() {
        "s3" | "s3a" => {
            let mut builder = AmazonS3Builder::from_env().with_url(url);
            for (key, value) in configured::<AmazonS3ConfigKey>(&config.options)? {
                builder = builder.with_config(key, value);
            }
            Arc::new(builder.build().map_err(|error| error.to_string())?)
        }
        "gs" => {
            let mut builder = GoogleCloudStorageBuilder::from_env().with_url(url);
            for (key, value) in configured::<GoogleConfigKey>(&config.options)? {
                builder = builder.with_config(key, value);
            }
            Arc::new(builder.build().map_err(|error| error.to_string())?)
        }
        "az" | "azure" | "abfs" | "abfss" => {
            let mut builder = MicrosoftAzureBuilder::from_env().with_url(url);
            for (key, value) in configured::<AzureConfigKey>(&config.options)? {
                builder = builder.with_config(key, value);
            }
            Arc::new(builder.build().map_err(|error| error.to_string())?)
        }
        "file" | "memory" => {
            return Err(
                "images.url must be an object store (s3://, gs://, az://): instances are \
                 stateless, so images cannot live on one of them"
                    .to_owned(),
            );
        }
        other => return Err(format!("images.url: unsupported scheme `{other}:`")),
    };
    let prefix = parsed.path().trim_matches('/');
    let store = if prefix.is_empty() {
        store
    } else {
        Arc::new(PrefixStore::new(store, prefix))
    };
    Ok(Some(ImageStorage {
        store,
        public_base_url: config
            .public_base_url
            .as_ref()
            .map(|base| base.trim_end_matches('/').to_owned()),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn images(url: &str) -> ImagesConfig {
        ImagesConfig {
            url: Some(url.to_owned()),
            ..ImagesConfig::default()
        }
    }

    #[test]
    fn only_object_stores_are_accepted() {
        assert!(connect(&ImagesConfig::default()).unwrap().is_none());
        for local in ["file:///var/lib/kippu/images", "memory:///"] {
            let error = connect(&images(local)).unwrap_err();
            assert!(error.contains("stateless"), "{error}");
        }
        assert!(connect(&images("ftp://example.org/x")).is_err());
        let mut config = images("s3://kippu-images/prod");
        config
            .options
            .insert("aws_region".to_owned(), "eu-west-1".to_owned().into());
        assert!(connect(&config).unwrap().is_some());
        config
            .options
            .insert("not_a_setting".to_owned(), "x".to_owned().into());
        assert!(connect(&config).is_err());
    }
}
