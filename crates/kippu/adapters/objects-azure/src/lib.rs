#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

use kippu_objects_common::{ObjectStoreAdapter, Settings};
use kippu_store::BoxError;
use object_store::azure::{AzureConfigKey, MicrosoftAzureBuilder};

/// Connects to the Azure Blob Storage location `url` names. `options` are the provider's
/// settings (`images.options`); anything they leave out is read from the environment.
pub fn connect(url: &str, options: &[(String, String)]) -> Result<ObjectStoreAdapter, BoxError> {
    kippu_objects_common::connect(url, |url| {
        let mut builder = MicrosoftAzureBuilder::from_env().with_url(url);
        for (key, value) in Settings(options).parse::<AzureConfigKey>()? {
            builder = builder.with_config(key, value);
        }
        Ok(builder.build()?)
    })
}
