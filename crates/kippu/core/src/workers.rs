//! Runs background tasks until shutdown.

use crate::app::AppState;
use crate::module::{BackgroundTask, Progress};
use tokio_util::sync::CancellationToken;

/// Runs `task` repeatedly: immediately again while it reports more work, after its interval
/// otherwise. Failures are logged and retried after the interval; they never stop the task.
pub(crate) async fn run(task: BackgroundTask, state: AppState, shutdown: CancellationToken) {
    tracing::info!(task = task.name(), "background task started");
    loop {
        let rest = match task.run_once(state.clone()).await {
            Ok(Progress::MoreWork) => None,
            Ok(Progress::Idle) => Some(task.interval()),
            Err(error) => {
                tracing::warn!(task = task.name(), %error, "background task failed; retrying");
                Some(task.interval())
            }
        };
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
