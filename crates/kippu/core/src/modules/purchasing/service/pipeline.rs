//! The pipeline behind the front door: workers turn queued requests into reservations or
//! rejections, and expire reservations left unpaid.

use kippu_domain::catalog::{Sale, TicketType};
use kippu_domain::outbox::IntegrationEvent;
use kippu_domain::purchase::{Basket, PurchaseRequest, PurchaseStatus, RejectionReason};
use kippu_domain::reservation::{Reservation, ReservationStatus, ReservedItem};
use kippu_domain::validation::IdempotencyKey;
use kippu_domain::{AccountId, Duration, Money, PurchaseRequestId, ReservationId};
use kippu_store::{BoxError, Insertion, Lease, LineItems, StoreTx};
use tracing::Instrument;

use crate::app::AppState;
use crate::error::{ApiError, ApiResult, ProblemKind, StatusCode};
use crate::module::Progress;
use crate::modules::payments::service::{holds, line_items};

/// The answer to resubmitting a key: the same purchase again, or a different one.
fn resubmitted(existing: PurchaseRequest, basket: &Basket) -> ApiResult<PurchaseRequest> {
    if existing.basket == *basket {
        tracing::info!(id = %existing.id, "purchase request already queued");
        Ok(existing)
    } else {
        Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            ProblemKind::IDEMPOTENCY_KEY_REUSED,
            "this Idempotency-Key was already used for a different purchase",
        ))
    }
}

/// Records a purchase request. Submitting the same key again returns the same request.
///
/// With an inbox the request is accepted into it and persisted by a worker later, unless the
/// database already has it. A retry while the first attempt is still in the inbox is
/// dropped by the inbox's deduplication; if its basket differed, the first one wins.
#[tracing::instrument(skip_all)]
pub(super) async fn submit(
    state: &AppState,
    account: AccountId,
    sale: &Sale,
    key: &IdempotencyKey,
    basket: Basket,
) -> ApiResult<PurchaseRequest> {
    let now = state.now();
    let request = PurchaseRequest {
        id: PurchaseRequestId::derive(account, sale.id, key),
        account_id: account,
        sale_id: sale.id,
        basket,
        status: PurchaseStatus::Queued,
        created_at: now,
        updated_at: now,
    };
    if let Some(inbox) = state.inbox() {
        if let Some(existing) = state.store().purchase_request(request.id).await? {
            return resubmitted(existing, &request.basket);
        }
        inbox.enqueue(&request).await?;
        tracing::info!(id = %request.id, "purchase request accepted into the inbox");
        return Ok(request);
    }
    match state.store().insert_purchase_request(&request).await? {
        Insertion::Inserted => {
            tracing::info!(id = %request.id, "purchase request queued");
            Ok(request)
        }
        Insertion::Existing(existing) => resubmitted(existing, &request.basket),
    }
}

/// Why a request cannot be fulfilled, judged from the catalog alone.
fn precheck(
    request: &PurchaseRequest,
    sale: &Sale,
    ticket_types: &[TicketType],
) -> Option<RejectionReason> {
    // A request is judged by when it was accepted, not by how long the queue took.
    if !sale.is_open_at(request.created_at) {
        return Some(RejectionReason::SaleClosed);
    }
    if request.basket.items().iter().any(|item| {
        !ticket_types
            .iter()
            .any(|ticket_type| ticket_type.id == item.ticket_type_id)
    }) {
        return Some(RejectionReason::UnknownTicketType);
    }
    if request.basket.total_quantity() > u64::from(sale.max_tickets_per_request) {
        return Some(RejectionReason::LimitExceeded);
    }
    None
}

fn reserved_items(
    request: &PurchaseRequest,
    ticket_types: &[TicketType],
) -> ApiResult<(Vec<ReservedItem>, Money)> {
    let items = request
        .basket
        .items()
        .iter()
        .map(|item| {
            let ticket_type = ticket_types
                .iter()
                .find(|ticket_type| ticket_type.id == item.ticket_type_id)
                .ok_or_else(|| ApiError::internal("ticket type vanished"))?;
            Ok(ReservedItem {
                ticket_type_id: item.ticket_type_id,
                quantity: item.quantity,
                unit_price: ticket_type.price,
            })
        })
        .collect::<ApiResult<Vec<_>>>()?;
    let total = ReservedItem::total(&items).map_err(ApiError::internal)?;
    Ok((items, total))
}

async fn reject(
    tx: &mut dyn StoreTx,
    request: &PurchaseRequest,
    reason: RejectionReason,
    state: &AppState,
) -> ApiResult<()> {
    tx.purchases()
        .set_purchase_status(request.id, PurchaseStatus::Rejected { reason }, state.now())
        .await?;
    tracing::info!(request = %request.id, ?reason, "purchase request rejected");
    Ok(())
}

/// Processes one queued request: reserves its tickets or rejects it, in one transaction.
/// Processing a request that is no longer queued does nothing, so redelivery is harmless.
#[tracing::instrument(skip_all)]
pub(crate) async fn process(state: &AppState, request: &PurchaseRequest) -> ApiResult<()> {
    let store = state.store();
    let sale = store
        .sale(request.sale_id)
        .await?
        .ok_or_else(|| ApiError::not_found("sale"))?;
    let ticket_types = store.list_ticket_types(sale.id).await?;
    let event = store
        .event(sale.event_id)
        .await?
        .ok_or_else(|| ApiError::not_found("event"))?;
    let denied = store
        .account_denied(event.organization_id, event.id, request.account_id)
        .await?;

    let mut tx = store.begin().await?;
    let Some(current) = tx.purchases().lock_purchase_request(request.id).await? else {
        return Ok(());
    };
    if current.status != PurchaseStatus::Queued {
        return Ok(());
    }
    let rejection = if denied {
        Some(RejectionReason::AccountDenied)
    } else {
        precheck(&current, &sale, &ticket_types)
    };
    if let Some(reason) = rejection {
        reject(&mut *tx, &current, reason, state).await?;
        return Ok(tx.commit().await?);
    }

    let (items, total) = reserved_items(&current, &ticket_types)?;
    let lines = LineItems::from(&current.basket);
    if !tx
        .inventory()
        .try_take_quota(current.account_id, &holds(&lines, &ticket_types)?)
        .await?
    {
        reject(&mut *tx, &current, RejectionReason::LimitExceeded, state).await?;
        return Ok(tx.commit().await?);
    }
    if !tx.inventory().try_hold(&lines).await? {
        tx.inventory()
            .return_quota(current.account_id, &lines)
            .await?;
        reject(&mut *tx, &current, RejectionReason::SoldOut, state).await?;
        return Ok(tx.commit().await?);
    }

    let now = state.now();
    let reservation = Reservation {
        id: ReservationId::generate(),
        purchase_request_id: current.id,
        account_id: current.account_id,
        sale_id: sale.id,
        event_id: sale.event_id,
        items,
        total,
        environment: sale.environment,
        status: ReservationStatus::Reserved,
        attestor_id: None,
        expires_at: now + Duration::seconds(i64::from(sale.reservation_ttl_seconds)),
        created_at: now,
        updated_at: now,
    };
    tx.reservations().insert_reservation(&reservation).await?;
    tx.purchases()
        .set_purchase_status(
            current.id,
            PurchaseStatus::Reserved {
                reservation_id: reservation.id,
            },
            now,
        )
        .await?;
    tx.commit().await?;
    tracing::info!(
        request = %current.id,
        reservation = %reservation.id,
        total = %reservation.total,
        "tickets reserved"
    );
    Ok(())
}

/// Processes requests one after another, in the order given.
async fn process_in_order(state: &AppState, requests: &[PurchaseRequest]) {
    for request in requests {
        if let Err(error) = process(state, request).await {
            // The lease lapses and another attempt picks the request up.
            tracing::warn!(request = %request.id, %error, "purchase request not processed");
        }
    }
}

/// Splits a batch (oldest first) by sale, keeping each sale's requests in order.
fn by_sale(requests: Vec<PurchaseRequest>) -> Vec<Vec<PurchaseRequest>> {
    let mut groups: Vec<Vec<PurchaseRequest>> = Vec::new();
    for request in requests {
        match groups.iter_mut().find(|group| {
            group
                .first()
                .is_some_and(|first| first.sale_id == request.sale_id)
        }) {
            Some(group) => group.push(request),
            None => groups.push(vec![request]),
        }
    }
    groups
}

/// Background task: claims a batch of queued requests and processes them.
///
/// Requests of one sale compete for the same stock, so they are always processed in the order
/// they were accepted. When the store allows concurrent writers, up to
/// `workers.purchase_concurrency` sales are processed at once.
#[tracing::instrument(skip_all)]
pub(crate) async fn process_batch(state: AppState) -> Result<Progress, BoxError> {
    let workers = &state.config().workers;
    let now = state.now();
    let lease = Lease {
        now,
        until: now + Duration::seconds(i64::from(workers.purchase_lease_seconds.get())),
    };
    let batch_size = workers.purchase_batch_size.get();
    let claimed = state
        .store()
        .claim_purchase_requests(lease, batch_size)
        .await?;
    let full = claimed.len() >= batch_size as usize;
    let concurrency = if state.store().capabilities().concurrent_writers {
        workers.purchase_concurrency.get() as usize
    } else {
        1
    };

    if concurrency == 1 {
        process_in_order(&state, &claimed).await;
    } else {
        let mut running = tokio::task::JoinSet::new();
        for group in by_sale(claimed) {
            if running.len() >= concurrency {
                running.join_next().await;
            }
            let state = state.clone();
            running.spawn(
                async move { process_in_order(&state, &group).await }
                    .instrument(tracing::Span::current()),
            );
        }
        while running.join_next().await.is_some() {}
    }
    Ok(if full {
        Progress::MoreWork
    } else {
        Progress::Idle
    })
}

/// Releases what a reservation holds and records its new status.
pub(super) async fn release(tx: &mut dyn StoreTx, reservation: &Reservation) -> ApiResult<()> {
    let items = line_items(reservation);
    // Quota before stock, each in ticket type order: the order purchases take them in, so a
    // cancellation or an expiry cannot deadlock with the same buyer's purchase.
    tx.inventory()
        .return_quota(reservation.account_id, &items)
        .await?;
    tx.inventory().release_held(&items).await?;
    tx.reservations().update_reservation(reservation).await?;
    Ok(())
}

/// Expires one overdue reservation. Does nothing if it was paid or released meanwhile.
#[tracing::instrument(skip_all)]
pub(crate) async fn expire(state: &AppState, id: ReservationId) -> ApiResult<()> {
    let now = state.now();
    let mut tx = state.store().begin().await?;
    let Some(mut reservation) = tx.reservations().lock_reservation(id).await? else {
        return Ok(());
    };
    if !reservation.is_overdue(now) {
        return Ok(());
    }
    reservation.status = reservation.status.expire()?;
    reservation.updated_at = now;
    release(&mut *tx, &reservation).await?;
    tx.outbox()
        .append_event(
            &IntegrationEvent::ReservationExpired { reservation_id: id },
            now,
        )
        .await?;
    tx.commit().await?;
    tracing::info!(reservation = %id, "reservation expired");
    Ok(())
}

/// Background task: expires a batch of overdue reservations.
#[tracing::instrument(skip_all)]
pub(crate) async fn expire_batch(state: AppState) -> Result<Progress, BoxError> {
    const BATCH: u32 = 100;
    let overdue = state
        .store()
        .overdue_reservations(state.now(), BATCH)
        .await?;
    for id in &overdue {
        if let Err(error) = expire(&state, *id).await {
            tracing::warn!(reservation = %id, %error, "reservation not expired");
        }
    }
    Ok(if overdue.len() >= BATCH as usize {
        Progress::MoreWork
    } else {
        Progress::Idle
    })
}

#[cfg(test)]
mod tests {
    use kippu_domain::purchase::LineItem;
    use kippu_domain::{SaleId, TicketTypeId, Timestamp};

    use super::*;

    fn request(sale: SaleId, second: i64) -> PurchaseRequest {
        let at = Timestamp::from_unix_seconds(second);
        PurchaseRequest {
            id: PurchaseRequestId::generate(),
            account_id: AccountId::generate(),
            sale_id: sale,
            basket: Basket::new(vec![LineItem {
                ticket_type_id: TicketTypeId::generate(),
                quantity: 1,
            }])
            .unwrap(),
            status: PurchaseStatus::Queued,
            created_at: at,
            updated_at: at,
        }
    }

    #[test]
    fn batches_split_by_sale_keep_arrival_order() {
        let (a, b) = (SaleId::generate(), SaleId::generate());
        let batch = vec![request(a, 1), request(b, 2), request(a, 3), request(b, 4)];
        let ids: Vec<_> = batch.iter().map(|request| request.id).collect();
        let groups = by_sale(batch);
        let grouped: Vec<Vec<_>> = groups
            .iter()
            .map(|group| group.iter().map(|request| request.id).collect())
            .collect();
        assert_eq!(grouped, vec![vec![ids[0], ids[2]], vec![ids[1], ids[3]]]);
    }
}
