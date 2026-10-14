//! The pipeline behind the front door: workers turn queued requests into reservations or
//! rejections, and expire reservations left unpaid.

use std::collections::HashSet;

use kippu_domain::catalog::{EventSummary, Sale, TicketType};
use kippu_domain::outbox::IntegrationEvent;
use kippu_domain::purchase::{Basket, PurchaseRequest, PurchaseStatus, RejectionReason};
use kippu_domain::reservation::{Reservation, ReservationStatus, ReservedItem};
use kippu_domain::validation::IdempotencyKey;
use kippu_domain::{AccountId, Duration, Money, PurchaseRequestId, ReservationId, SaleId};
use kippu_store::{BoxError, Insertion, LineItems, QueuedRequest, StoreTx};
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
    event: &EventSummary,
    sale: &Sale,
    ticket_types: &[TicketType],
) -> Option<RejectionReason> {
    // A request is judged by when it was accepted, not by how long the queue took; but an
    // event cancelled or withdrawn meanwhile sells nothing more.
    if !event.is_on_sale() || !sale.is_open_at(request.created_at) {
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

/// Serves a run of one sale's queued requests, none of the same account, in the transaction
/// that holds the sale's queue: reserves their tickets or rejects them, as serving them one by
/// one in arrival order would.
///
/// Every quota is taken first, in account order, then the stock of every type the run needs
/// is locked in ticket type order, and only then is stock taken in arrival order. The queue,
/// then quota, then stock, each in a fixed order, is the lock order every transaction follows
/// (no other takes a queue), so no two transactions deadlock. Accounts being distinct, no
/// request's quota depends on another's, so the outcomes are those of the one-by-one order.
/// One commit serves the whole run.
#[tracing::instrument(skip_all)]
async fn serve_run(
    mut tx: Box<dyn StoreTx>,
    state: &AppState,
    catalog: &SaleCatalog,
    run: &[QueuedRequest],
) -> ApiResult<()> {
    let decisions = decide(&mut *tx, catalog, run).await?;
    let settled = settle(&mut *tx, state, &catalog.sale, run, decisions).await?;
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

/// The first phase: in account order, rejects each request or takes its quota. Decisions come
/// back in the run's order.
async fn decide(
    tx: &mut dyn StoreTx,
    catalog: &SaleCatalog,
    run: &[QueuedRequest],
) -> ApiResult<Vec<Decision>> {
    let mut by_account: Vec<usize> = (0..run.len()).collect();
    by_account.sort_by_key(|&index| run[index].request.account_id);
    let mut decisions = Vec::with_capacity(run.len());
    for index in by_account {
        let QueuedRequest {
            request,
            account_denied,
        } = &run[index];
        let rejection = if *account_denied {
            Some(RejectionReason::AccountDenied)
        } else {
            precheck(
                request,
                &catalog.event,
                &catalog.sale,
                &catalog.ticket_types,
            )
        };
        let decision = if let Some(reason) = rejection {
            Decision::Rejected(reason)
        } else {
            let (items, total) = reserved_items(request, &catalog.ticket_types)?;
            let lines = LineItems::from(&request.basket);
            let quota = holds(&lines, &catalog.ticket_types)?;
            if tx
                .inventory()
                .try_take_quota(request.account_id, &quota)
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
        };
        decisions.push((index, decision));
    }
    decisions.sort_by_key(|&(index, _)| index);
    Ok(decisions
        .into_iter()
        .map(|(_, decision)| decision)
        .collect())
}

/// The second phase: takes stock in arrival order and records every request's outcome.
async fn settle(
    tx: &mut dyn StoreTx,
    state: &AppState,
    sale: &Sale,
    run: &[QueuedRequest],
    decisions: Vec<Decision>,
) -> ApiResult<Vec<(PurchaseRequestId, Settled)>> {
    // Stock is taken in arrival order, so one request's types may come before an earlier
    // one's: every type is locked first, in ticket type order, as all other transactions do.
    // A single request takes its own types in that order anyway.
    let holding: Vec<&LineItems> = decisions
        .iter()
        .filter_map(|decision| match decision {
            Decision::Quota { lines, .. } => Some(lines),
            Decision::Rejected(_) => None,
        })
        .collect();
    if holding.len() > 1 {
        let stock = LineItems::new(holding.iter().flat_map(|lines| lines.iter().copied()));
        tx.inventory().lock_stock(&stock).await?;
    }

    let mut settled = Vec::with_capacity(run.len());
    for (QueuedRequest { request, .. }, decision) in run.iter().zip(decisions) {
        let now = state.now();
        let outcome = match decision {
            Decision::Rejected(reason) => Settled::Rejected(reason),
            Decision::Quota {
                items,
                total,
                lines,
            } => {
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

/// How many of a sale's oldest requests can be served together: those before the first
/// account that repeats, whose quota depends on its earlier request.
fn first_run(queued: &[QueuedRequest]) -> usize {
    let mut accounts = HashSet::new();
    queued
        .iter()
        .take_while(|queued| accounts.insert(queued.request.account_id))
        .count()
}

/// What one transaction made of a sale's queue.
enum Turn {
    /// Another transaction holds the queue.
    Held,
    /// Nothing is due.
    Empty,
    /// Served `count` requests; `more` if requests may be left behind them.
    Served { count: usize, more: bool },
    /// Serving these requests failed and was rolled back.
    Failed {
        requests: Vec<PurchaseRequestId>,
        error: ApiError,
    },
}

/// Takes a sale's queue, reads its oldest due requests (at most `limit`) and serves the first
/// run of them. Whatever ends the transaction lets the queue go.
async fn take_turn(state: &AppState, catalog: &SaleCatalog, limit: u32) -> ApiResult<Turn> {
    let mut tx = state.store().begin().await?;
    if !tx.purchases().lock_purchase_queue(catalog.sale.id).await? {
        return Ok(Turn::Held);
    }
    let queued = tx
        .purchases()
        .queued_requests(
            catalog.sale.id,
            catalog.event.id,
            catalog.event.organization_id,
            state.now(),
            limit,
        )
        .await?;
    let run = &queued[..first_run(&queued)];
    if run.is_empty() {
        return Ok(Turn::Empty);
    }
    let more =
        queued.len() > run.len() || u32::try_from(queued.len()).is_ok_and(|read| read >= limit);
    Ok(match serve_run(tx, state, catalog, run).await {
        Ok(()) => Turn::Served {
            count: run.len(),
            more,
        },
        Err(error) => Turn::Failed {
            requests: run.iter().map(|queued| queued.request.id).collect(),
            error,
        },
    })
}

/// How far a worker got with a sale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Served {
    /// Another worker holds the sale's queue and serves it.
    Elsewhere,
    /// Nothing due is left, or serving failed: no hurry to come back.
    All,
    /// The batch ran out with requests left.
    Partly,
}

/// Serves up to a batch of one sale's queued requests, a run per transaction, in arrival
/// order. A run that fails is redone one request at a time; a request that fails on its own
/// is postponed by `workers.purchase_lease_seconds`, so it holds up nobody behind it.
async fn serve_sale(state: &AppState, sale_id: SaleId) -> Served {
    let catalog = match sale_catalog(state, sale_id).await {
        Ok(catalog) => catalog,
        Err(error) => {
            tracing::warn!(sale = %sale_id, %error, "purchase requests not processed");
            return Served::All;
        }
    };
    let workers = &state.config().workers;
    let mut left = workers.purchase_batch_size.get();
    let mut one_by_one: usize = 0;
    while left > 0 {
        let limit = if one_by_one > 0 { 1 } else { left };
        let turn = match take_turn(state, &catalog, limit).await {
            Ok(turn) => turn,
            Err(error) => {
                tracing::warn!(sale = %sale_id, %error, "purchase requests not processed");
                return Served::All;
            }
        };
        match turn {
            Turn::Held => return Served::Elsewhere,
            Turn::Empty => return Served::All,
            Turn::Served { count, more } => {
                left = left.saturating_sub(u32::try_from(count).unwrap_or(u32::MAX));
                one_by_one = one_by_one.saturating_sub(count);
                if !more {
                    return Served::All;
                }
            }
            Turn::Failed { requests, error } => {
                if let [request] = requests.as_slice() {
                    tracing::warn!(%request, %error, "purchase request not processed; it is tried again later");
                    let until = state.now()
                        + Duration::seconds(i64::from(workers.purchase_lease_seconds.get()));
                    if let Err(error) = state
                        .store()
                        .postpone_purchase_request(*request, until)
                        .await
                    {
                        tracing::warn!(%request, %error, "purchase request not postponed");
                        return Served::All;
                    }
                    left -= 1;
                    one_by_one = one_by_one.saturating_sub(1);
                } else {
                    tracing::warn!(
                        requests = requests.len(),
                        %error,
                        "purchase requests processed together failed; processing them one by one"
                    );
                    one_by_one = requests.len();
                }
            }
        }
    }
    Served::Partly
}

/// Background task: serves the sales that have queued requests, the one with the oldest
/// request first.
///
/// A sale's requests are read in arrival order inside the transaction that holds its purchase
/// queue, so each sale is served first come, first served however many workers run on however
/// many instances, and a worker that dies holds nothing once its transaction is gone. When the
/// store allows concurrent writers, up to `workers.purchase_concurrency` sales are served at
/// once.
#[tracing::instrument(skip_all)]
pub(crate) async fn process_batch(state: AppState) -> Result<Progress, BoxError> {
    let workers = &state.config().workers;
    let batch_size = workers.purchase_batch_size.get();
    let sales = state
        .store()
        .sales_with_queued_requests(state.now(), batch_size)
        .await?;
    // A full batch to look at may have left sales out.
    let full = sales
        .iter()
        .map(|&(_, queued)| u64::from(queued))
        .sum::<u64>()
        >= u64::from(batch_size);
    let concurrency = if state.store().capabilities().concurrent_writers {
        workers.purchase_concurrency.get() as usize
    } else {
        1
    };

    let mut outcomes = Vec::with_capacity(sales.len());
    if concurrency == 1 {
        for (sale, _) in sales {
            outcomes.push(serve_sale(&state, sale).await);
        }
    } else {
        let mut running = tokio::task::JoinSet::new();
        for (sale, _) in sales {
            if running.len() >= concurrency
                && let Some(served) = running.join_next().await
            {
                outcomes.push(served.unwrap_or(Served::All));
            }
            let state = state.clone();
            running.spawn(
                async move { serve_sale(&state, sale).await }.instrument(tracing::Span::current()),
            );
        }
        while let Some(served) = running.join_next().await {
            outcomes.push(served.unwrap_or(Served::All));
        }
    }
    // Come back at once while requests are left, or while sales may lie beyond a full look;
    // not for sales other workers are serving (that would only spin on their queues).
    let more = outcomes.contains(&Served::Partly) || (full && outcomes.contains(&Served::All));
    Ok(if more {
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
    fn the_first_run_ends_before_a_repeated_account() {
        let sale = SaleId::generate();
        let (a, b, c) = (
            AccountId::generate(),
            AccountId::generate(),
            AccountId::generate(),
        );
        let queue: Vec<_> = [a, b, c, a, b]
            .into_iter()
            .enumerate()
            .map(|(second, account)| QueuedRequest {
                request: PurchaseRequest {
                    account_id: account,
                    ..request(sale, i64::try_from(second).unwrap())
                },
                account_denied: false,
            })
            .collect();
        assert_eq!(first_run(&queue), 3);
        assert_eq!(first_run(&queue[3..]), 2);
        assert_eq!(first_run(&[]), 0);
    }
}
