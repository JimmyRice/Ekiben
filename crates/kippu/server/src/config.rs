//! Merging configuration sources: defaults, then the file, then the environment, then flags.

use std::path::Path;

use figment::Figment;
use figment::providers::{Env, Format, Serialized, Toml};
use kippu_core::Config;
use kippu_core::config::DatabaseConfig;

/// Configuration could not be loaded.
#[derive(Debug, thiserror::Error)]
#[error("invalid configuration: {0}")]
pub struct ConfigError(#[from] Box<figment::Error>);

/// The configuration sources, before command-line overrides.
///
/// `KIPPU_DATABASE__URL=…` sets `database.url`; `__` separates sections. Values are parsed as
/// TOML, so lists work too: `KIPPU_ROOT__KEYS='[{name="alice", public_key="…"}]'`.
pub fn sources(file: Option<&Path>) -> Figment {
    let figment = Figment::new();
    let figment = match file {
        Some(path) => figment.merge(Toml::file_exact(path)),
        None => figment,
    };
    figment.merge(Env::prefixed("KIPPU_").split("__"))
}

/// Adds a command-line override such as `("database.url", url)`.
pub fn with_override<T: serde::Serialize>(
    figment: Figment,
    key: &str,
    value: Option<T>,
) -> Figment {
    match value {
        Some(value) => figment.merge(Serialized::default(key, value)),
        None => figment,
    }
}

/// Extracts the full configuration.
pub fn extract(figment: &Figment) -> Result<Config, ConfigError> {
    figment
        .extract()
        .map_err(|error| ConfigError(Box::new(error)))
}

/// Extracts only the `database` section, for commands such as `kippu migrate` that need
/// nothing else (in particular, no signing keys).
pub fn extract_database(figment: &Figment) -> Result<DatabaseConfig, ConfigError> {
    figment
        .extract_inner("database")
        .map_err(|error| ConfigError(Box::new(error)))
}

#[cfg(test)]
mod tests {
    use secrecy::ExposeSecret;

    use super::*;

    #[test]
    fn the_database_section_alone_is_enough_to_migrate() {
        let figment = with_override(Figment::new(), "database.url", Some("sqlite://kippu.db"));
        assert!(
            extract(&figment).is_err(),
            "the full configuration needs keys"
        );
        let database = extract_database(&figment).unwrap();
        assert_eq!(database.url.expose_secret(), "sqlite://kippu.db");
        assert!(extract_database(&Figment::new()).is_err());
    }
}
