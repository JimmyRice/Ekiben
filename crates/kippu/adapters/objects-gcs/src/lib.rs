#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

use kippu_objects_common::{ObjectStoreAdapter, Settings};
use kippu_store::BoxError;
use object_store::gcp::{GoogleCloudStorageBuilder, GoogleConfigKey};

/// Connects to the Google Cloud Storage location `url` names. `options` are the provider's
/// settings (`images.options`); anything they leave out is read from the environment.
pub fn connect(url: &str, options: &[(String, String)]) -> Result<ObjectStoreAdapter, BoxError> {
    kippu_objects_common::connect(url, |url| {
        let mut builder = GoogleCloudStorageBuilder::from_env().with_url(url);
        for (key, value) in Settings(options).parse::<GoogleConfigKey>()? {
            builder = builder.with_config(key, value);
        }
        Ok(builder.build()?)
    })
}
