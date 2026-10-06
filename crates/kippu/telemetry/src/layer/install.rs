//! Installing the global logger.

use std::io::IsTerminal as _;

use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;

use super::{Format, LogLayer};

/// What `RUST_LOG` defaults to: Kippu's milestones, plus the database statements that make
/// up each request's `db` line.
const DEFAULT_FILTER: &str = "info,sqlx::query=debug";

/// Installs the global logger. `RUST_LOG` filters as usual; without it, `info` plus the
/// statements counted in every request. Statements are never listed one by one, only counted,
/// and the slow ones (sqlx warns after a second) show up as warnings.
pub fn init(format: Format) {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));
    let colors = format == Format::Pretty
        && std::io::stdout().is_terminal()
        && std::env::var_os("NO_COLOR").is_none_or(|value| value.is_empty());
    let layer = LogLayer::new(format, std::io::stdout).colors(colors);
    // Already installed (e.g. by a test harness): keep the existing logger.
    drop(
        tracing_subscriber::registry()
            .with(filter)
            .with(layer)
            .try_init(),
    );
}
