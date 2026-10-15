//! The pipeline behind the front door: workers turn queued requests into reservations or
//! rejections, and expire reservations left unpaid.

use std::collections::BTreeMap;

use kippu_domain::catalog::{Audience, EventSummary, Sale, TicketType};
use kippu_domain::outbox::IntegrationEvent;
use kippu_domain::purchase::{Basket, PurchaseRequest, PurchaseStatus, RejectionReason};
use kippu_domain::reservation::{Reservation, ReservationStatus, ReservedItem};
use kippu_domain::validation::IdempotencyKey;
use kippu_domain::{
    AccountId, Duration, Money, PurchaseRequestId, ReservationId, SaleId, TicketTypeId,
};
use kippu_store::{BoxError, Holds, Insertion, LineItems, QueuedRequest, QuotaLines, StoreTx};
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

/// What a request reserves, at the prices of its ticket types, and what that costs.
pub(super) fn reserved_items(
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

/// A request the deny lists and the catalog allow: what it reserves if its quota and the stock
/// allow too.
struct Candidate {
    items: Vec<ReservedItem>,
    total: Money,
    lines: LineItems,
    quota: Holds,
}

/// Where a request stands while its run is decided.
enum Decision {
    /// Rejected.
    Rejected(RejectionReason),
    /// Gets its tickets, unless its quota or the stock rule it out.
    Reserve(Candidate),
}

/// What became of a request, for the log once its transaction is committed.
enum Settled {
    Rejected(RejectionReason),
    Reserved {
        reservation: ReservationId,
        total: Money,
    },
}

/// Serves a run of one sale's queued requests in the transaction that holds the sale's queue:
/// reserves their tickets or rejects them, as serving them one by one in arrival order would,
/// an account's later requests included.
///
/// The quotas the run needs are locked first, in account order, then its stock, in ticket type
/// order. The queue, then quota, then stock, each in a fixed order, is the lock order every
/// transaction follows (no other takes a queue), so no two transactions deadlock. However long
/// the run, a few statements decide and record it, and one commit serves it.
#[tracing::instrument(skip_all)]
async fn serve_run(
    mut tx: Box<dyn StoreTx>,
    state: &AppState,
    catalog: &SaleCatalog,
    run: &[QueuedRequest],
) -> ApiResult<()> {
    let mut decisions = run
        .iter()
        .map(|queued| check(queued, catalog))
        .collect::<ApiResult<Vec<_>>>()?;
    take(&mut *tx, run, &mut decisions).await?;
    let settled = record(&mut *tx, state, &catalog.sale, run, decisions).await?;
    tx.commit().await?;

    for (request, outcome) in settled {
        match outcome {
            Settled::Rejected(reason) => {
                tracing::info!(%request, ?reason, "purchase request rejected");
            }
            Settled::Reserved { reservation, total } => {
                tracing::info!(%request, %reservation, %total, "tickets reserved");
            }
        }
    }
    Ok(())
}

/// Rejects a request on what the deny lists, the invitation list and the catalog say, or
/// makes it a candidate.
fn check(queued: &QueuedRequest, catalog: &SaleCatalog) -> ApiResult<Decision> {
    let QueuedRequest {
        request,
        account_denied,
        account_invited,
    } = queued;
    let rejection = if *account_denied {
        Some(RejectionReason::AccountDenied)
    } else if !account_invited {
        Some(RejectionReason::NotInvited)
    } else {
        precheck(
            request,
            &catalog.event,
            &catalog.sale,
            &catalog.ticket_types,
        )
    };
    if let Some(reason) = rejection {
        return Ok(Decision::Rejected(reason));
    }
    let (items, total) = reserved_items(request, &catalog.ticket_types)?;
    let lines = LineItems::from(&request.basket);
    let quota = holds(&lines, &catalog.ticket_types)?;
    Ok(Decision::Reserve(Candidate {
        items,
        total,
        lines,
        quota,
    }))
}

/// A run's candidates with their accounts, in arrival order.
fn candidates<'a>(
    run: &'a [QueuedRequest],
    decisions: &'a [Decision],
) -> impl Iterator<Item = (AccountId, &'a Candidate)> {
    run.iter()
        .zip(decisions)
        .filter_map(|(queued, decision)| match decision {
            Decision::Reserve(candidate) => Some((queued.request.account_id, candidate)),
            Decision::Rejected(_) => None,
        })
}

/// Takes quota and stock for a run's candidates, rejecting those over a limit or without
/// stock.
async fn take(
    tx: &mut dyn StoreTx,
    run: &[QueuedRequest],
    decisions: &mut [Decision],
) -> ApiResult<()> {
    let mut reserving = decisions
        .iter()
        .enumerate()
        .filter(|(_, decision)| matches!(decision, Decision::Reserve(_)))
        .map(|(index, _)| index);
    match (reserving.next(), reserving.next()) {
        (None, _) => Ok(()),
        (Some(index), None) => {
            take_alone(tx, run[index].request.account_id, &mut decisions[index]).await
        }
        (Some(_), Some(_)) => take_together(tx, run, decisions).await,
    }
}

/// A lone candidate needs nothing read: the conditional writes decide it, quota first, then
/// stock.
async fn take_alone(
    tx: &mut dyn StoreTx,
    account: AccountId,
    decision: &mut Decision,
) -> ApiResult<()> {
    let Decision::Reserve(candidate) = decision else {
        return Ok(());
    };
    let rejection = if !tx
        .inventory()
        .try_take_quota(account, &candidate.quota)
        .await?
    {
        Some(RejectionReason::LimitExceeded)
    } else if tx.inventory().try_hold(&candidate.lines).await? {
        None
    } else {
        tx.inventory()
            .return_quota(account, &candidate.lines)
            .await?;
        Some(RejectionReason::SoldOut)
    };
    if let Some(reason) = rejection {
        *decision = Decision::Rejected(reason);
    }
    Ok(())
}

/// Several candidates are decided on what is read under lock: their quotas, then their stock,
/// are locked and read, [`decide_in_order`] decides them, and what the reserved ones take is
/// written at once.
async fn take_together(
    tx: &mut dyn StoreTx,
    run: &[QueuedRequest],
    decisions: &mut [Decision],
) -> ApiResult<()> {
    let quota = QuotaLines::new(
        candidates(run, decisions)
            .flat_map(|(account, candidate)| QuotaLines::of(account, &candidate.lines)),
    );
    let stock = LineItems::new(
        candidates(run, decisions).flat_map(|(_, candidate)| candidate.lines.iter().copied()),
    );
    let read = tx.inventory().lock_quotas(&quota).await?;
    let taken = quota
        .iter()
        .map(|line| (line.account_id, line.ticket_type_id))
        .zip(read)
        .collect();
    let read = tx.inventory().lock_stock(&stock).await?;
    let available = stock
        .iter()
        .map(|line| line.ticket_type_id)
        .zip(read)
        .collect();
    decide_in_order(run, decisions, taken, available);

    let quota = QuotaLines::new(
        candidates(run, decisions)
            .flat_map(|(account, candidate)| QuotaLines::of(account, &candidate.lines)),
    );
    if quota.is_empty() {
        return Ok(());
    }
    tx.inventory().take_quotas(&quota).await?;
    let stock = LineItems::new(
        candidates(run, decisions).flat_map(|(_, candidate)| candidate.lines.iter().copied()),
    );
    if tx.inventory().try_hold(&stock).await? {
        Ok(())
    } else {
        Err(ApiError::internal("stock changed while it was locked"))
    }
}

/// Decides candidates in arrival order on what each account has `taken` of each ticket type
/// and the stock `available`, as serving them one by one would: a candidate over a limit is
/// rejected, then one without stock, and only those reserved count against both, so an
/// account's later requests see what its earlier ones took.
fn decide_in_order(
    run: &[QueuedRequest],
    decisions: &mut [Decision],
    mut taken: BTreeMap<(AccountId, TicketTypeId), u32>,
    mut available: BTreeMap<TicketTypeId, u32>,
) {
    for (queued, decision) in run.iter().zip(decisions.iter_mut()) {
        let Decision::Reserve(candidate) = decision else {
            continue;
        };
        let account = queued.request.account_id;
        let within_limits = candidate.quota.iter().all(|hold| {
            taken
                .get(&(account, hold.ticket_type_id))
                .and_then(|taken| taken.checked_add(hold.quantity))
                .is_some_and(|after| after <= hold.per_account_limit)
        });
        let in_stock = candidate.lines.iter().all(|line| {
            available
                .get(&line.ticket_type_id)
                .is_some_and(|&left| line.quantity <= left)
        });
        if !within_limits {
            *decision = Decision::Rejected(RejectionReason::LimitExceeded);
        } else if !in_stock {
            *decision = Decision::Rejected(RejectionReason::SoldOut);
        } else {
            for hold in &candidate.quota {
                if let Some(taken) = taken.get_mut(&(account, hold.ticket_type_id)) {
                    *taken += hold.quantity;
                }
            }
            for line in &candidate.lines {
                if let Some(left) = available.get_mut(&line.ticket_type_id) {
                    *left -= line.quantity;
                }
            }
        }
    }
}

/// Records a run's outcomes: the reservations of the requests that got their tickets, and
/// every request's status.
async fn record(
    tx: &mut dyn StoreTx,
    state: &AppState,
    sale: &Sale,
    run: &[QueuedRequest],
    decisions: Vec<Decision>,
) -> ApiResult<Vec<(PurchaseRequestId, Settled)>> {
    let now = state.now();
    let mut reservations = Vec::new();
    let mut statuses = Vec::with_capacity(run.len());
    let mut settled = Vec::with_capacity(run.len());
    for (QueuedRequest { request, .. }, decision) in run.iter().zip(decisions) {
        let (status, outcome) = match decision {
            Decision::Rejected(reason) => (
                PurchaseStatus::Rejected { reason },
                Settled::Rejected(reason),
            ),
            Decision::Reserve(Candidate { items, total, .. }) => {
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
                    expires_at: now + Duration::seconds(i64::from(sale.reservation_ttl_seconds)),
                    created_at: now,
                    updated_at: now,
                };
                let reserved = (
                    PurchaseStatus::Reserved {
                        reservation_id: reservation.id,
                    },
                    Settled::Reserved {
                        reservation: reservation.id,
                        total,
                    },
                );
                reservations.push(reservation);
                reserved
            }
        };
        statuses.push((request.id, status));
        settled.push((request.id, outcome));
    }
    if !reservations.is_empty() {
        tx.reservations().insert_reservations(&reservations).await?;
    }
    tx.purchases().set_purchase_statuses(&statuses, now).await?;
    Ok(settled)
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

/// Takes a sale's queue, reads its oldest due requests (at most `limit`) and serves them as one
/// run. Whatever ends the transaction lets the queue go.
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
            catalog.sale.audience == Audience::Invited,
            state.now(),
            limit,
        )
        .await?;
    let run = queued.as_slice();
    if run.is_empty() {
        return Ok(Turn::Empty);
    }
    let more = u32::try_from(run.len()).is_ok_and(|read| read >= limit);
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

/// Serves up to a batch of one sale's queued requests, in arrival order, in one transaction. A run that fails is redone one request at a time; a request that fails on its own
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

    fn candidate(ticket_type: TicketTypeId, quantity: u32) -> Decision {
        let price = Money::new(1_000, kippu_domain::Currency::JPY).unwrap();
        let lines = LineItems::new([LineItem {
            ticket_type_id: ticket_type,
            quantity,
        }]);
        Decision::Reserve(Candidate {
            items: vec![ReservedItem {
                ticket_type_id: ticket_type,
                quantity,
                unit_price: price,
            }],
            total: price,
            quota: Holds::new(&lines, |_| Some(2)).unwrap(),
            lines,
        })
    }

    fn outcome(decision: &Decision) -> Option<RejectionReason> {
        match decision {
            Decision::Rejected(reason) => Some(*reason),
            Decision::Reserve(_) => None,
        }
    }

    #[test]
    fn a_run_is_decided_as_one_by_one_an_accounts_later_requests_included() {
        let sale = SaleId::generate();
        let ticket_type = TicketTypeId::generate();
        let (a, b, c) = (
            AccountId::generate(),
            AccountId::generate(),
            AccountId::generate(),
        );
        // Three tickets, two per account; `c` took two before.
        let arrivals = [(b, 2), (a, 2), (a, 1), (a, 2), (c, 1)];
        let run: Vec<_> = arrivals
            .iter()
            .enumerate()
            .map(|(second, &(account, _))| QueuedRequest {
                request: PurchaseRequest {
                    account_id: account,
                    ..request(sale, i64::try_from(second).unwrap())
                },
                account_denied: false,
                account_invited: true,
            })
            .collect();
        let mut decisions: Vec<_> = arrivals
            .iter()
            .map(|&(_, quantity)| candidate(ticket_type, quantity))
            .collect();
        let taken = [a, b, c]
            .into_iter()
            .map(|account| ((account, ticket_type), if account == c { 2 } else { 0 }))
            .collect();
        decide_in_order(
            &run,
            &mut decisions,
            taken,
            BTreeMap::from([(ticket_type, 3)]),
        );
        assert_eq!(
            decisions.iter().map(outcome).collect::<Vec<_>>(),
            [
                None,
                // A pair no longer fits, and takes no quota…
                Some(RejectionReason::SoldOut),
                // …so the same account's single is within its limit and gets the last ticket.
                None,
                // Then it is over its limit: quota is judged before stock.
                Some(RejectionReason::LimitExceeded),
                Some(RejectionReason::LimitExceeded),
            ]
        );
    }
}
