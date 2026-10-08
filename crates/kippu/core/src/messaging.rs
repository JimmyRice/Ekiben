//! Moving purchase requests from the inbox to the database, and events from the outbox to the
//! bus. Both run on every instance at once: persisting is idempotent by request id, and the
//! bus drops sequences it already has.

use std::time::Duration;

use kippu_store::{BoxError, Insertion};

use crate::app::AppState;
use crate::config::Config;
use crate::module::{BackgroundTask, Progress};

/// The tasks for whichever of the inbox and the bus this instance has.
pub(crate) fn tasks(config: &Config, inbox: bool, bus: bool) -> Vec<BackgroundTask> {
    let interval = Duration::from_millis(config.queue.interval_ms.get());
    let mut tasks = Vec::new();
    if inbox {
        tasks.push(BackgroundTask::every(
            "purchase-inbox",
            interval,
            persist_inbox,
        ));
    }
    if bus {
        tasks.push(BackgroundTask::every("event-relay", interval, relay_events));
    }
    tasks
}

/// Background task: moves a batch of accepted purchase requests into the database, where the
/// purchase workers claim them as usual, acknowledging each once it is stored.
async fn persist_inbox(state: AppState) -> Result<Progress, BoxError> {
    let Some(inbox) = state.inbox() else {
        return Ok(Progress::Idle);
    };
    let limit = state.config().queue.inbox_batch_size.get() as usize;
    let deliveries = inbox.receive(limit).await?;
    let full = deliveries.len() >= limit;
    let mut persisted = 0_usize;
    for delivery in deliveries {
        // Either outcome means the database has it; a retried enqueue is simply `Existing`.
        match state
            .store()
            .insert_purchase_request(delivery.request())
            .await?
        {
            Insertion::Inserted => persisted += 1,
            Insertion::Existing(_) => {}
        }
        delivery.ack().await?;
    }
    if persisted > 0 {
        tracing::info!(persisted, "purchase requests moved from the inbox");
    }
    Ok(if full {
        Progress::MoreWork
    } else {
        Progress::Idle
    })
}

/// Background task: publishes the outbox events the bus does not have yet, in order.
async fn relay_events(state: AppState) -> Result<Progress, BoxError> {
    let Some(bus) = state.event_bus() else {
        return Ok(Progress::Idle);
    };
    let limit = state.config().queue.relay_batch_size.get();
    let after = bus.last_published().await?;
    let records = state.store().outbox_after(after, limit).await?;
    for record in &records {
        bus.publish(record).await?;
    }
    if let Some(last) = records.last() {
        tracing::info!(
            through = last.sequence,
            published = records.len(),
            "events relayed"
        );
    }
    Ok(if records.len() >= limit as usize {
        Progress::MoreWork
    } else {
        Progress::Idle
    })
}
