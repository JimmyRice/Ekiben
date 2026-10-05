//! Logging.

use tracing_subscriber::EnvFilter;

use crate::cli::LogFormat;

/// Installs the global logger. `RUST_LOG` filters as usual (default `info`).
pub(crate) fn init(format: LogFormat) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let builder = tracing_subscriber::fmt().with_env_filter(filter);
    let result = match format {
        LogFormat::Pretty => builder.try_init(),
        LogFormat::Json => builder.json().try_init(),
    };
    // Already installed (e.g. by a test harness): keep the existing logger.
    drop(result);
}
