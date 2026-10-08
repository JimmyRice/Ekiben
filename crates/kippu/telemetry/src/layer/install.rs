//! Installing the global logger.

use std::io::IsTerminal as _;

use tracing_appender::non_blocking::{NonBlockingBuilder, WorkerGuard};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;

use super::{Format, LogLayer};

/// What `RUST_LOG` defaults to: Kippu's milestones, plus the database statements that make
/// up each request's `db` line.
const DEFAULT_FILTER: &str = "info,sqlx::query=debug";

/// Keeps the log writer running; dropping it writes out what is still queued. Hold it until
/// the process ends.
#[must_use = "dropping the guard stops the log writer"]
pub struct LogGuard(#[expect(dead_code, reason = "held for its Drop")] WorkerGuard);

/// Installs the global logger. `RUST_LOG` filters as usual; without it, `info` plus the
/// statements counted in every request. Statements are never listed one by one, only counted,
/// and the slow ones (sqlx warns after a second) show up as warnings.
///
/// Output goes to stdout from a thread of its own, so a slow log collector never stalls the
/// threads serving requests; only when it falls this far behind
/// ([`DEFAULT_BUFFERED_LINES_LIMIT`](tracing_appender::non_blocking::DEFAULT_BUFFERED_LINES_LIMIT)
/// units) does logging wait rather than drop lines.
pub fn init(format: Format) -> LogGuard {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));
    let colors = format == Format::Pretty
        && std::io::stdout().is_terminal()
        && std::env::var_os("NO_COLOR").is_none_or(|value| value.is_empty());
    let (writer, guard) = NonBlockingBuilder::default()
        .lossy(false)
        .thread_name("kippu-log")
        .finish(std::io::stdout());
    let layer = LogLayer::new(format, writer).colors(colors);
    // Already installed (e.g. by a test harness): keep the existing logger.
    drop(
        tracing_subscriber::registry()
            .with(filter)
            .with(layer)
            .try_init(),
    );
    LogGuard(guard)
}
