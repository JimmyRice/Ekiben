//! Calls to things outside the process.

use std::fmt::Display;

use tracing::Instrument;
use tracing::field::{Empty, display};

use crate::DEPENDENCY_SPAN;

/// Runs a call to something outside the process — a queue, object storage, a webhook
/// receiver — as a step of the current call chain: `system` and `operation` name it, the log
/// shows how long it took, and a failure shows its error. The result is passed through.
///
/// ```ignore
/// kippu_telemetry::call("nats", "enqueue", self.publish(subject, payload)).await
/// ```
pub async fn call<T, E: Display>(
    system: &'static str,
    operation: &'static str,
    future: impl Future<Output = Result<T, E>>,
) -> Result<T, E> {
    let span = tracing::info_span!(DEPENDENCY_SPAN, system, operation, error = Empty);
    let result = future.instrument(span.clone()).await;
    if let Err(error) = &result {
        span.record("error", display(error));
    }
    result
}
