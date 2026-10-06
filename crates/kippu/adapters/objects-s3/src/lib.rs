#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

use kippu_objects_common::{ObjectStoreAdapter, Settings};
use kippu_store::BoxError;
use object_store::aws::{AmazonS3Builder, AmazonS3ConfigKey};

/// Connects to the Amazon S3 and S3-compatible services location `url` names. `options` are the provider's
/// settings (`images.options`); anything they leave out is read from the environment.
pub fn connect(url: &str, options: &[(String, String)]) -> Result<ObjectStoreAdapter, BoxError> {
    kippu_objects_common::connect(url, |url| {
        let mut builder = AmazonS3Builder::from_env().with_url(url);
        for (key, value) in Settings(options).parse::<AmazonS3ConfigKey>()? {
            builder = builder.with_config(key, value);
        }
        Ok(builder.build()?)
    })
}
