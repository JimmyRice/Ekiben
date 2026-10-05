//! Runs background tasks until shutdown.

use tokio_util::sync::CancellationToken;
use tracing::Instrument;

use crate::app::AppState;
use crate::http::trace::{record_outcome, task_span};
use crate::module::{BackgroundTask, Progress};

/// Runs `task` repeatedly: immediately again while it reports more work, after its interval
/// otherwise. Failures are logged and retried after the interval; they never stop the task.
pub(crate) async fn run(task: BackgroundTask, state: AppState, shutdown: CancellationToken) {
    tracing::info!(task = task.name(), "background task started");
    loop {
        let span = task_span(task.name());
        let result = task.run_once(state.clone()).instrument(span.clone()).await;
        let rest = span.in_scope(|| match result {
            Ok(Progress::MoreWork) => {
                record_outcome(&span, "more work");
                None
            }
            Ok(Progress::Idle) => {
                record_outcome(&span, "idle");
                Some(task.interval())
            }
            Err(error) => {
                record_outcome(&span, "failed");
                tracing::warn!(%error, "background task failed; retrying");
                Some(task.interval())
            }
        });
        // Close the span now, not after the rest: its log entry ends here.
        drop(span);
        let wait = async {
            match rest {
                Some(interval) => tokio::time::sleep(interval).await,
                None => tokio::task::yield_now().await,
            }
        };
        tokio::select! {
            () = shutdown.cancelled() => break,
            () = wait => {}
        }
    }
    tracing::info!(task = task.name(), "background task stopped");
}
