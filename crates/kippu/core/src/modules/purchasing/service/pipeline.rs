//! The pipeline behind the front door: workers turn queued requests into reservations or
//! rejections, and expire reservations left unpaid.

use std::collections::HashSet;

use kippu_domain::catalog::{EventSummary, Sale, TicketType};
use kippu_domain::outbox::IntegrationEvent;
use kippu_domain::purchase::{Basket, PurchaseRequest, PurchaseStatus, RejectionReason};
use kippu_domain::reservation::{Reservation, ReservationStatus, ReservedItem};
use kippu_domain::validation::IdempotencyKey;
use kippu_domain::{AccountId, Duration, Money, PurchaseRequestId, ReservationId, SaleId};
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

/// What processing a sale's requests reads of the catalog, once per batch.
struct SaleCatalog {
    sale: Sale,
    ticket_types: Vec<TicketType>,
    event: EventSummary,
}

async fn sale_catalog(state: &AppState, sale_id: SaleId) -> ApiResult<SaleCatalog> {
    let store = state.store();
    let sale = store
        .sale(sale_id)
        .await?
        .ok_or_else(|| ApiError::not_found("sale"))?;
    let ticket_types = store.list_ticket_types(sale.id).await?;
    let event = store
        .event_summary(sale.event_id)
        .await?
        .ok_or_else(|| ApiError::not_found("event"))?;
    Ok(SaleCatalog {
        sale,
        ticket_types,
        event,
    })
}

/// What the first phase of processing decided for a request.
enum Decision {
    /// Rejected without touching stock.
    Rejected(RejectionReason),
    /// Its quota is taken; stock decides.
    Quota {
        items: Vec<ReservedItem>,
        total: Money,
        lines: LineItems,
    },
}

/// What became of a request, for the log once its transaction is committed.
enum Settled {
    Rejected(RejectionReason),
    Reserved(Reservation),
}

/// Processes queued requests of one sale, none of the same account, in one transaction:
/// reserves their tickets or rejects them, as processing them one by one in the given order
/// would. Requests no longer queued are skipped, so redelivery is harmless.
///
/// Every quota is taken first, in account order, then the stock of every type the group needs
/// is locked in ticket type order, and only then is stock taken in the given order: quota
/// before stock, each in a fixed order, is the lock order every transaction follows, so two
/// workers' groups cannot deadlock. Accounts being distinct, no request's quota depends
/// on another's, so the outcomes are those of the one-by-one order. One commit serves the
/// whole group.
#[tracing::instrument(skip_all)]
async fn process_group(
    state: &AppState,
    catalog: &SaleCatalog,
    requests: &[PurchaseRequest],
) -> ApiResult<()> {
    let store = state.store();
    let mut denied = Vec::with_capacity(requests.len());
    for request in requests {
        denied.push(
            store
                .account_denied(
                    catalog.event.organization_id,
                    catalog.event.id,
                    request.account_id,
                )
                .await?,
        );
    }
    let mut tx = store.begin().await?;
    let decisions = decide(&mut *tx, catalog, requests, &denied).await?;
    let settled = settle(&mut *tx, state, &catalog.sale, requests, decisions).await?;
    tx.commit().await?;

    for (request, outcome) in settled {
        match outcome {
            Settled::Rejected(reason) => {
                tracing::info!(%request, ?reason, "purchase request rejected");
            }
            Settled::Reserved(reservation) => tracing::info!(
                %request,
                reservation = %reservation.id,
                total = %reservation.total,
                "tickets reserved"
            ),
        }
    }
    Ok(())
}

/// The first phase: locks each request, in account order, and rejects it or takes its quota.
/// `None` for a request that is no longer queued.
async fn decide(
    tx: &mut dyn StoreTx,
    catalog: &SaleCatalog,
    requests: &[PurchaseRequest],
    denied: &[bool],
) -> ApiResult<Vec<Option<Decision>>> {
    let mut by_account: Vec<usize> = (0..requests.len()).collect();
    by_account.sort_by_key(|&index| requests[index].account_id);
    let mut decisions: Vec<Option<Decision>> = requests.iter().map(|_| None).collect();
    for index in by_account {
        let Some(current) = tx
            .purchases()
            .lock_purchase_request(requests[index].id)
            .await?
        else {
            continue;
        };
        if current.status != PurchaseStatus::Queued {
            continue;
        }
        let rejection = if denied[index] {
            Some(RejectionReason::AccountDenied)
        } else {
            precheck(&current, &catalog.sale, &catalog.ticket_types)
        };
        decisions[index] = Some(if let Some(reason) = rejection {
            Decision::Rejected(reason)
        } else {
            let (items, total) = reserved_items(&current, &catalog.ticket_types)?;
            let lines = LineItems::from(&current.basket);
            let quota = holds(&lines, &catalog.ticket_types)?;
            if tx
                .inventory()
                .try_take_quota(current.account_id, &quota)
                .await?
            {
                Decision::Quota {
                    items,
                    total,
                    lines,
                }
            } else {
                Decision::Rejected(RejectionReason::LimitExceeded)
            }
        });
    }
    Ok(decisions)
}

/// The second phase: takes stock in arrival order and records every request's outcome.
async fn settle(
    tx: &mut dyn StoreTx,
    state: &AppState,
    sale: &Sale,
    requests: &[PurchaseRequest],
    decisions: Vec<Option<Decision>>,
) -> ApiResult<Vec<(PurchaseRequestId, Settled)>> {
    // Stock is taken in arrival order, so one request's types may come before an earlier
    // one's: every type is locked first, in ticket type order, as all other transactions do.
    // A single request takes its own types in that order anyway.
    let holding: Vec<&LineItems> = decisions
        .iter()
        .flatten()
        .filter_map(|decision| match decision {
            Decision::Quota { lines, .. } => Some(lines),
            Decision::Rejected(_) => None,
        })
        .collect();
    if holding.len() > 1 {
        let stock = LineItems::new(holding.iter().flat_map(|lines| lines.iter().copied()));
        tx.inventory().lock_stock(&stock).await?;
    }

    let mut settled = Vec::with_capacity(requests.len());
    for (request, decision) in requests.iter().zip(decisions) {
        let now = state.now();
        let outcome = match decision {
            None => continue,
            Some(Decision::Rejected(reason)) => Settled::Rejected(reason),
            Some(Decision::Quota {
                items,
                total,
                lines,
            }) => {
                if tx.inventory().try_hold(&lines).await? {
                    let reservation = Reservation {
                        id: ReservationId::generate(),
                        purchase_request_id: request.id,
                        account_id: request.account_id,
                        sale_id: sale.id,
                        event_id: sale.event_id,
                        items,
                        total,
                        environment: sale.environment,
                        status: ReservationStatus::Reserved,
                        attestor_id: None,
                        expires_at: now
                            + Duration::seconds(i64::from(sale.reservation_ttl_seconds)),
                        created_at: now,
                        updated_at: now,
                    };
                    tx.reservations().insert_reservation(&reservation).await?;
                    Settled::Reserved(reservation)
                } else {
                    tx.inventory()
                        .return_quota(request.account_id, &lines)
                        .await?;
                    Settled::Rejected(RejectionReason::SoldOut)
                }
            }
        };
        let status = match &outcome {
            Settled::Rejected(reason) => PurchaseStatus::Rejected { reason: *reason },
            Settled::Reserved(reservation) => PurchaseStatus::Reserved {
                reservation_id: reservation.id,
            },
        };
        tx.purchases()
            .set_purchase_status(request.id, status, now)
            .await?;
        settled.push((request.id, outcome));
    }
    Ok(settled)
}

/// Splits one sale's requests, in order, into runs in which no account occurs twice: the
/// groups [`process_group`] can decide together.
fn runs(requests: &[PurchaseRequest]) -> Vec<&[PurchaseRequest]> {
    let mut runs = Vec::new();
    let mut start = 0;
    let mut accounts = HashSet::new();
    for (index, request) in requests.iter().enumerate() {
        if !accounts.insert(request.account_id) {
            runs.push(&requests[start..index]);
            start = index;
            accounts.clear();
            accounts.insert(request.account_id);
        }
    }
    if start < requests.len() {
        runs.push(&requests[start..]);
    }
    runs
}

/// Processes one sale's requests in the order given, reading the catalog once for all of
/// them and committing each run of distinct accounts at once. A group that fails is rolled
/// back and processed again one request at a time, so one bad request delays no other.
async fn process_in_order(state: &AppState, requests: &[PurchaseRequest]) {
    let Some(first) = requests.first() else {
        return;
    };
    // The lease lapses and another attempt picks the requests up.
    let catalog = match sale_catalog(state, first.sale_id).await {
        Ok(catalog) => catalog,
        Err(error) => {
            tracing::warn!(sale = %first.sale_id, %error, "purchase requests not processed");
            return;
        }
    };
    for run in runs(requests) {
        let Err(error) = process_group(state, &catalog, run).await else {
            continue;
        };
        if let [request] = run {
            tracing::warn!(request = %request.id, %error, "purchase request not processed");
            continue;
        }
        tracing::warn!(
            requests = run.len(),
            %error,
            "purchase requests processed together failed; processing them one by one"
        );
        for request in run {
            if let Err(error) = process_group(state, &catalog, std::slice::from_ref(request)).await
            {
                tracing::warn!(request = %request.id, %error, "purchase request not processed");
            }
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
        for group in by_sale(claimed) {
            process_in_order(&state, &group).await;
        }
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
    fn runs_never_repeat_an_account_and_keep_arrival_order() {
        let sale = SaleId::generate();
        let (a, b, c) = (
            AccountId::generate(),
            AccountId::generate(),
            AccountId::generate(),
        );
        let requests: Vec<_> = [a, b, a, c, b, b]
            .into_iter()
            .enumerate()
            .map(|(second, account)| PurchaseRequest {
                account_id: account,
                ..request(sale, i64::try_from(second).unwrap())
            })
            .collect();
        let accounts: Vec<Vec<_>> = runs(&requests)
            .iter()
            .map(|run| run.iter().map(|request| request.account_id).collect())
            .collect();
        assert_eq!(accounts, vec![vec![a, b], vec![a, c, b], vec![b]]);
        assert_eq!(runs(&[]), Vec::<&[PurchaseRequest]>::new());
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
