//! Reading the outbox as a feed: attestors see what concerns them, operators everything.

use kippu_domain::outbox::IntegrationEvent;
use kippu_domain::payment::Attestor;
use kippu_store::OutboxRecord;

use crate::app::AppState;
use crate::auth::{Principal, Scope};
use crate::error::ApiResult;
use crate::modules::payments::permissions::FEED_READ;

/// The integration events after `after` addressed to `attestor` (`payment.requested`,
/// `refund.required`), reading at most `limit` events of the outbox.
#[tracing::instrument(skip_all)]
pub async fn attestor_feed(
    state: &AppState,
    attestor: &Attestor,
    after: i64,
    limit: u32,
) -> ApiResult<Vec<OutboxRecord>> {
    let records = state.store().outbox_after(after, limit).await?;
    Ok(records
        .into_iter()
        .filter(|record| match &record.event {
            IntegrationEvent::PaymentRequested { attestor_id, .. }
            | IntegrationEvent::RefundRequired { attestor_id, .. } => *attestor_id == attestor.id,
            _ => false,
        })
        .collect())
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
