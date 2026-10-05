//! The purchase pipeline.
//!
//! ```text
//! submit ──▶ Queued (durable) ──worker──▶ Reserved (inventory held) ──checkout──▶ PaymentPending
//!                                  └────▶ Rejected (sold out, limit, …)
//! ```
//!
//! Submitting only records intent, so the front door stays fast under any load; workers drain
//! the queue at a steady pace, and the database alone decides who gets a ticket.

use kippu_domain::catalog::{Sale, TicketType};
use kippu_domain::outbox::IntegrationEvent;
use kippu_domain::payment::Environment;
use kippu_domain::purchase::{Basket, PurchaseRequest, PurchaseStatus, RejectionReason};
use kippu_domain::reservation::{Reservation, ReservationStatus, ReservedItem};
use kippu_domain::validation::IdempotencyKey;
use kippu_domain::{AccountId, AttestorId, Duration, Money, PurchaseRequestId, ReservationId};
use kippu_store::{BoxError, Hold, Insertion, Lease, StoreTx};

use crate::app::AppState;
use crate::error::{ApiError, ApiResult};
use crate::module::Progress;
use crate::modules::payments::service::{IncomingPayment, line_items, settle};

/// Records a purchase request. Submitting the same key again returns the same request.
pub(crate) async fn submit(
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
    match state.store().insert_purchase_request(&request).await? {
        Insertion::Inserted => {
            tracing::info!(id = %request.id, "purchase request queued");
            Ok(request)
        }
        Insertion::Existing(existing) if existing.basket == request.basket => {
            tracing::info!(id = %existing.id, "purchase request already queued");
            Ok(existing)
        }
        Insertion::Existing(_) => Err(ApiError::new(
            axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            "idempotency-key-reused",
            "this Idempotency-Key was already used for a different purchase",
        )),
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
    let mut items = Vec::new();
    let mut total: Option<Money> = None;
    for item in request.basket.items() {
        let ticket_type = ticket_types
            .iter()
            .find(|ticket_type| ticket_type.id == item.ticket_type_id)
            .ok_or_else(|| ApiError::internal("ticket type vanished"))?;
        let line = ticket_type.price.checked_mul(item.quantity);
        total = match (total, line) {
            (None, Some(line)) => Some(line),
            (Some(sum), Some(line)) => sum.checked_add(line),
            _ => None,
        };
        items.push(ReservedItem {
            ticket_type_id: item.ticket_type_id,
            quantity: item.quantity,
            unit_price: ticket_type.price,
        });
    }
    let total =
        total.ok_or_else(|| ApiError::internal("ticket prices mix currencies or overflow"))?;
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
pub(crate) async fn process(state: &AppState, request: &PurchaseRequest) -> ApiResult<()> {
    let store = state.store();
    let sale = store
        .sale(request.sale_id)
        .await?
        .ok_or_else(|| ApiError::not_found("sale"))?;
    let ticket_types = store.list_ticket_types(sale.id).await?;

    let mut tx = store.begin().await?;
    let Some(current) = tx.purchases().lock_purchase_request(request.id).await? else {
        return Ok(());
    };
    if current.status != PurchaseStatus::Queued {
        return Ok(());
    }
    if let Some(reason) = precheck(&current, &sale, &ticket_types) {
        reject(&mut *tx, &current, reason, state).await?;
        return Ok(tx.commit().await?);
    }

    let (items, total) = reserved_items(&current, &ticket_types)?;
    let holds: Vec<Hold> = current
        .basket
        .items()
        .iter()
        .map(|item| Hold {
            ticket_type_id: item.ticket_type_id,
            quantity: item.quantity,
            per_account_limit: ticket_types
                .iter()
                .find(|ticket_type| ticket_type.id == item.ticket_type_id)
                .map_or(0, |ticket_type| ticket_type.per_account_limit),
        })
        .collect();
    if !tx
        .inventory()
        .try_take_quota(current.account_id, &holds)
        .await?
    {
        reject(&mut *tx, &current, RejectionReason::LimitExceeded, state).await?;
        return Ok(tx.commit().await?);
    }
    if !tx.inventory().try_hold(current.basket.items()).await? {
        tx.inventory()
            .return_quota(current.account_id, current.basket.items())
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

/// Background task: claims a batch of queued requests and processes them in order.
pub(crate) async fn process_batch(state: AppState) -> Result<Progress, BoxError> {
    let workers = &state.config().workers;
    let now = state.now();
    let lease = Lease {
        now,
        until: now + Duration::seconds(i64::from(workers.purchase_lease_seconds)),
    };
    let batch_size = workers.purchase_batch_size;
    let claimed = state
        .store()
        .claim_purchase_requests(lease, batch_size)
        .await?;
    for request in &claimed {
        if let Err(error) = process(&state, request).await {
            // The lease lapses and another attempt picks the request up.
            tracing::warn!(request = %request.id, %error, "purchase request not processed");
        }
    }
    Ok(if claimed.len() >= batch_size as usize {
        Progress::MoreWork
    } else {
        Progress::Idle
    })
}

/// Releases what a reservation holds and records its new status.
async fn release(tx: &mut dyn StoreTx, reservation: &Reservation) -> ApiResult<()> {
    let items = line_items(reservation);
    tx.inventory().release_held(&items).await?;
    tx.inventory()
        .return_quota(reservation.account_id, &items)
        .await?;
    tx.reservations().update_reservation(reservation).await?;
    Ok(())
}

/// Expires one overdue reservation. Does nothing if it was paid or released meanwhile.
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

/// The buyer gives a reservation up. Idempotent.
pub(crate) async fn cancel(state: &AppState, reservation: &Reservation) -> ApiResult<Reservation> {
    let now = state.now();
    let mut tx = state.store().begin().await?;
    let mut current = tx
        .reservations()
        .lock_reservation(reservation.id)
        .await?
        .ok_or_else(|| ApiError::not_found("reservation"))?;
    let was_holding = current.status.holds_inventory();
    current.status = current.status.cancel()?;
    if was_holding {
        current.updated_at = now;
        release(&mut *tx, &current).await?;
    }
    tx.commit().await?;
    tracing::info!(reservation = %current.id, "reservation cancelled");
    Ok(current)
}

/// The buyer chooses how to pay. Free reservations are settled on the spot.
pub(crate) async fn checkout(
    state: &AppState,
    reservation: &Reservation,
    attestor_id: Option<AttestorId>,
) -> ApiResult<Reservation> {
    let store = state.store();
    if reservation.total.is_zero() {
        let free = store
            .attestor(AttestorId::FREE)
            .await?
            .ok_or_else(|| ApiError::internal("the built-in free attestor is missing"))?;
        let payment = IncomingPayment {
            attestation_id: format!("free:{}", reservation.id),
            reservation_id: reservation.id,
            amount: reservation.total,
            occurred_at: state.now(),
        };
        return Ok(settle(state, &free, payment).await?.reservation);
    }

    let attestor_id = attestor_id.ok_or_else(|| {
        kippu_domain::ValidationError::new(
            "attestor_id",
            "is required for a reservation that costs money",
        )
    })?;
    let sale = store
        .sale(reservation.sale_id)
        .await?
        .ok_or_else(|| ApiError::not_found("sale"))?;
    let attestor = store
        .attestor(attestor_id)
        .await?
        .filter(|attestor| !attestor.revoked && sale.accepts(attestor.id))
        .ok_or_else(|| {
            ApiError::new(
                axum::http::StatusCode::UNPROCESSABLE_ENTITY,
                "attestor-not-accepted",
                "this sale does not accept that payment method",
            )
        })?;
    if attestor.environment == Environment::Sandbox && reservation.environment == Environment::Live
    {
        return Err(ApiError::new(
            axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            "environment-mismatch",
            "a sandbox payment method cannot pay for a live sale",
        ));
    }

    let now = state.now();
    let mut tx = store.begin().await?;
    let mut current = tx
        .reservations()
        .lock_reservation(reservation.id)
        .await?
        .ok_or_else(|| ApiError::not_found("reservation"))?;
    current.status = current.status.checkout()?;
    current.attestor_id = Some(attestor.id);
    current.updated_at = now;
    tx.reservations().update_reservation(&current).await?;
    let event = IntegrationEvent::PaymentRequested {
        reservation_id: current.id,
        attestor_id: attestor.id,
        amount: current.total,
        expires_at: current.expires_at,
    };
    tx.outbox().append_event(&event, now).await?;
    tx.commit().await?;
    Ok(current)
}
