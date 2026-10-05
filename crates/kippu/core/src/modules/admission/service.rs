use kippu_domain::admission::AdmissionPolicy;
use kippu_store::BoxError;

use crate::app::AppState;
use crate::module::Progress;

/// Background task: advances every waiting room by one batch, unless its sale's purchase
/// backlog is already large. Safe on many instances: the advance is a compare-and-set.
pub(crate) async fn admit_batches(state: AppState) -> Result<Progress, BoxError> {
    let store = state.store();
    for sale_id in store.pending_waiting_rooms().await? {
        let Some(sale) = store.sale(sale_id).await? else {
            continue;
        };
        let AdmissionPolicy::WaitingRoom {
            admit_per_tick,
            max_backlog,
        } = sale.admission
        else {
            continue;
        };
        let room = store.waiting_room(sale_id).await?;
        let backlog = store.queued_purchase_count(sale_id).await?;
        let next = room.next_admitted_through(admit_per_tick, max_backlog, backlog);
        if next > room.admitted_through
            && store
                .advance_waiting_room(sale_id, room.admitted_through, next)
                .await?
        {
            tracing::info!(sale = %sale_id, admitted_through = next, backlog, "admitted a batch");
        }
    }
    Ok(Progress::Idle)
}
