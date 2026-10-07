//! Reading the outbox as a feed: attestors see what concerns them, operators everything.

use kippu_domain::outbox::IntegrationEvent;
use kippu_domain::payment::Attestor;
use kippu_store::OutboxRecord;

use crate::app::AppState;
use crate::auth::{Principal, Scope};
use crate::error::ApiResult;
use crate::modules::payments::permissions::FEED_READ;

/// How many windows of `limit` events one poll of the attestor feed reads at most.
const SCAN_WINDOWS: usize = 10;

/// One poll of an attestor's feed.
#[derive(Debug, Clone)]
pub struct AttestorFeed {
    /// The events addressed to the attestor, in sequence order.
    pub events: Vec<OutboxRecord>,
    /// How far the outbox was read: the sequence to poll after next. Every event addressed to
    /// the attestor up to it is in `events`, so it moves past others' events even when there
    /// are none for this attestor.
    pub position: i64,
}

fn addressed_to(record: &OutboxRecord, attestor: &Attestor) -> bool {
    match &record.event {
        IntegrationEvent::PaymentRequested { attestor_id, .. }
        | IntegrationEvent::RefundRequired { attestor_id, .. } => *attestor_id == attestor.id,
        _ => false,
    }
}

/// The integration events after `after` addressed to `attestor` (`payment.requested`,
/// `refund.required`), at most `limit` of them. The outbox holds every event of the
/// deployment, so this reads it in windows of `limit` — at most ten of them —
/// until `limit` events are found or the outbox ends, and reports how far it read.
#[tracing::instrument(skip_all)]
pub async fn attestor_feed(
    state: &AppState,
    attestor: &Attestor,
    after: i64,
    limit: u32,
) -> ApiResult<AttestorFeed> {
    let wanted = usize::try_from(limit).unwrap_or(usize::MAX);
    let mut feed = AttestorFeed {
        events: Vec::new(),
        position: after,
    };
    for _ in 0..SCAN_WINDOWS {
        let records = state.store().outbox_after(feed.position, limit).await?;
        let last_window = records.len() < wanted;
        for record in records {
            feed.position = record.sequence;
            if addressed_to(&record, attestor) {
                feed.events.push(record);
                if feed.events.len() >= wanted {
                    return Ok(feed);
                }
            }
        }
        if last_window {
            break;
        }
    }
    Ok(feed)
}

/// Every integration event after `after`, at most `limit` of them.
#[tracing::instrument(skip_all)]
pub async fn full_feed(
    state: &AppState,
    principal: &Principal,
    after: i64,
    limit: u32,
) -> ApiResult<Vec<OutboxRecord>> {
    state.authorize(principal, FEED_READ, Scope::Global)?;
    Ok(state.store().outbox_after(after, limit).await?)
}
