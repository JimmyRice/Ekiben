//! What buyers do: submit purchase requests, poll them, pay for or give up reservations.

use kippu_domain::admission::AdmissionPolicy;
use kippu_domain::outbox::IntegrationEvent;
use kippu_domain::payment::Environment;
use kippu_domain::purchase::{Basket, LineItem, PurchaseRequest};
use kippu_domain::reservation::Reservation;
use kippu_domain::validation::IdempotencyKey;
use kippu_domain::{AttestorId, PurchaseRequestId, ReservationId, SaleId, ValidationError};
use kippu_store::{Keyset, Page, PageRequest};

use super::pipeline::{release, submit};
use crate::app::AppState;
use crate::auth::{Principal, Scope};
use crate::error::{ApiError, ApiResult, StatusCode};
use crate::modules::catalog::service::visible_sale;
use crate::modules::payments::service::{IncomingPayment, settle};
use crate::modules::purchasing::permissions::{PURCHASES_CREATE, RESERVATIONS_READ};

/// A purchase a buyer asks for.
#[derive(Debug, Clone)]
pub struct PurchaseSubmission {
    /// The buyer's key for this purchase: resubmitting with it never buys twice. Required.
    pub idempotency_key: Option<String>,
    /// The pass from the waiting room; required by sales that have one.
    pub admission_pass: Option<String>,
    /// Ticket types and quantities.
    pub items: Vec<LineItem>,
}

/// A purchase request that was accepted.
#[derive(Debug, Clone)]
pub struct Submitted {
    /// The request, queued.
    pub request: PurchaseRequest,
    /// Vouches for the request while the database may not have it yet (with an inbox); polls
    /// carry it until then.
    pub receipt: Option<String>,
}

/// Asks to buy tickets: records a queued purchase request for a worker to fulfil or reject.
/// Submitting the same key again returns the same request.
#[tracing::instrument(skip_all)]
pub async fn submit_purchase(
    state: &AppState,
    principal: &Principal,
    sale_id: SaleId,
    submission: PurchaseSubmission,
) -> ApiResult<Submitted> {
    let account = principal.require_account()?;
    state.authorize(principal, PURCHASES_CREATE, Scope::Account(account))?;
    let key = submission
        .idempotency_key
        .ok_or(ValidationError::new(
            "Idempotency-Key",
            "header is required",
        ))
        .and_then(IdempotencyKey::new)?;
    let (sale, _) = visible_sale(state, Some(principal), sale_id).await?;
    let now = state.now();
    if !sale.is_open_at(now) {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "sale-closed",
            "this sale is not open",
        ));
    }
    if matches!(sale.admission, AdmissionPolicy::WaitingRoom { .. }) {
        let pass = submission.admission_pass.ok_or_else(|| {
            ApiError::new(
                StatusCode::FORBIDDEN,
                "admission-required",
                "join the waiting room first",
            )
        })?;
        state
            .tokens()
            .verify_admission_pass(&pass, account, sale.id, now)?;
    }
    let basket = Basket::new(submission.items)?;

    let request = submit(state, account, &sale, &key, basket).await?;
    // The database may not have it yet; the receipt answers polls until it does.
    let receipt = state
        .inbox()
        .map(|_| state.tokens().issue_purchase_receipt(&request));
    Ok(Submitted { request, receipt })
}

/// A purchase request of the caller's: `queued`, `reserved` or `rejected`. `receipt` is the
/// one it was submitted with, if any.
#[tracing::instrument(skip_all)]
pub async fn purchase_request(
    state: &AppState,
    principal: &Principal,
    id: PurchaseRequestId,
    receipt: Option<String>,
) -> ApiResult<PurchaseRequest> {
    let not_found = || ApiError::not_found("purchase request");
    let Some(request) = state.store().purchase_request(id).await? else {
        // Accepted into the inbox but not persisted yet: the receipt vouches for it.
        let account = principal.account_id().ok_or_else(not_found)?;
        return receipt
            .and_then(|receipt| {
                state
                    .tokens()
                    .verify_purchase_receipt(&receipt, id, account, state.now())
            })
            .ok_or_else(not_found);
    };
    if !state.policy().permits(
        principal,
        RESERVATIONS_READ,
        Scope::Account(request.account_id),
    ) {
        return Err(not_found());
    }
    Ok(request)
}

/// The caller's reservations, newest first.
#[tracing::instrument(skip_all)]
pub async fn reservations(
    state: &AppState,
    principal: &Principal,
    page: PageRequest<Keyset>,
) -> ApiResult<Page<Reservation, Keyset>> {
    let account = principal.require_account()?;
    state.authorize(principal, RESERVATIONS_READ, Scope::Account(account))?;
    let reservations = state
        .store()
        .reservations_for_account(account, page.plus_one())
        .await?;
    Ok(Page::from_lookahead(
        reservations,
        page.limit,
        |reservation| Keyset {
            at: reservation.created_at,
            id: reservation.id.as_uuid(),
        },
    ))
}

/// A reservation the caller may see: their own (admins see all). Others look missing.
#[tracing::instrument(skip_all)]
pub async fn reservation(
    state: &AppState,
    principal: &Principal,
    id: ReservationId,
) -> ApiResult<Reservation> {
    let reservation = state
        .store()
        .reservation(id)
        .await?
        .ok_or_else(|| ApiError::not_found("reservation"))?;
    if state.policy().permits(
        principal,
        RESERVATIONS_READ,
        Scope::Account(reservation.account_id),
    ) {
        Ok(reservation)
    } else {
        Err(ApiError::not_found("reservation"))
    }
}

/// The buyer gives a reservation up, releasing its tickets. Idempotent.
#[tracing::instrument(skip_all)]
pub async fn cancel(
    state: &AppState,
    principal: &Principal,
    id: ReservationId,
) -> ApiResult<Reservation> {
    let reservation = reservation(state, principal, id).await?;
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

/// The buyer chooses how to pay; the attestor is told through `payment.requested`. Free
/// reservations are settled on the spot and need no attestor.
#[tracing::instrument(skip_all)]
pub async fn checkout(
    state: &AppState,
    principal: &Principal,
    id: ReservationId,
    attestor_id: Option<AttestorId>,
) -> ApiResult<Reservation> {
    let reservation = reservation(state, principal, id).await?;
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
            occurred_at: None,
        };
        return Ok(settle(state, &free, payment).await?.reservation);
    }

    let attestor_id = attestor_id.ok_or_else(|| {
        ValidationError::new(
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
                StatusCode::UNPROCESSABLE_ENTITY,
                "attestor-not-accepted",
                "this sale does not accept that payment method",
            )
        })?;
    if attestor.environment == Environment::Sandbox && reservation.environment == Environment::Live
    {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
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
