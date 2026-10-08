//! Settlement: turning an attested payment into tickets — or into a refund obligation.
//!
//! Everything a payment changes is committed in **one** transaction: the attestation record,
//! the inventory, the reservation, the tickets and the outbox event. Either all of it happens
//! or none of it does, so money can never be recorded without its tickets (or its refund).

use kippu_domain::catalog::TicketType;
use kippu_domain::outbox::IntegrationEvent;
use kippu_domain::payment::{
    Attestor, Environment, PaymentAttestation, PaymentDisposition, validate_attestation_id,
};
use kippu_domain::purchase::LineItem;
use kippu_domain::refund::{Refund, RefundReason, RefundStatus};
use kippu_domain::reservation::{Reservation, Settlement};
use kippu_domain::ticket::TicketStatus;
use kippu_domain::{AttestorId, Money, RefundId, ReservationId, TicketId, Timestamp};
use kippu_store::{Holds, Insertion, LineItems, StoreTx};

use super::refunds::{new_refund, revoke};

use crate::app::AppState;
use crate::auth::{Principal, Scope};
use crate::error::{ApiError, ApiResult, StatusCode};
use crate::modules::payments::permissions::PAYMENTS_MANUAL;
use crate::modules::ticketing::service::issue_tickets;

/// What an attestor reports.
#[derive(Debug, Clone)]
pub enum AttestorReport {
    /// The buyer paid.
    Paid(IncomingPayment),
    /// The attestor returned money Kippu asked for with `refund.required`.
    Refunded {
        /// The attestor's reference for the payment.
        attestation_id: String,
        /// The refund of issued tickets it returned, as `refund.required` named it; `None`
        /// when the whole payment could not be used.
        refund_id: Option<RefundId>,
    },
    /// The whole payment went back without Kippu asking: a chargeback, or a full refund made
    /// in the provider's dashboard. The tickets it paid for are revoked.
    Reversed {
        /// The attestor's reference for the payment.
        attestation_id: String,
    },
}

/// A payment taken in person, e.g. cash at the venue.
#[derive(Debug, Clone)]
pub struct ManualPayment {
    /// The organizer's reference for the payment, e.g. a receipt number. Unique.
    pub attestation_id: String,
    /// The amount received; must equal the reservation total.
    pub amount: Money,
}

/// An attested payment as received.
#[derive(Debug, Clone)]
pub struct IncomingPayment {
    /// The attestor's own reference for the payment, unique per attestor.
    pub attestation_id: String,
    /// The reservation paid for.
    pub reservation_id: ReservationId,
    /// The amount paid; must equal the reservation total.
    pub amount: Money,
    /// When the payment happened; now if unknown.
    pub occurred_at: Option<Timestamp>,
}

/// What recording a payment (or a refund) did.
#[derive(Debug, Clone)]
pub struct PaymentResult {
    /// What became of the money.
    pub disposition: PaymentDisposition,
    /// The reservation afterwards.
    pub reservation: Reservation,
    /// Tickets issued for the reservation.
    pub ticket_ids: Vec<TicketId>,
    /// Whether the attestation had already been recorded, so nothing changed.
    pub replayed: bool,
    /// The refund of issued tickets the report confirmed or caused, if it concerned one.
    pub refund: Option<Refund>,
}

/// The reserved items as inventory line items.
pub(crate) fn line_items(reservation: &Reservation) -> LineItems {
    LineItems::new(reservation.items.iter().map(|item| LineItem {
        ticket_type_id: item.ticket_type_id,
        quantity: item.quantity,
    }))
}

/// `items` as quota claims under each ticket type's per-account limit.
pub(crate) fn holds(items: &LineItems, ticket_types: &[TicketType]) -> ApiResult<Holds> {
    Holds::new(items, |id| {
        ticket_types
            .iter()
            .find(|ticket_type| ticket_type.id == id)
            .map(|ticket_type| ticket_type.per_account_limit)
    })
    .ok_or_else(|| ApiError::internal("a reserved ticket type no longer exists"))
}

/// A reservation an attestor may charge for: one of a sale that accepts it. Others look
/// missing.
#[tracing::instrument(skip_all)]
pub async fn reservation_for_attestor(
    state: &AppState,
    attestor: &Attestor,
    reservation_id: ReservationId,
) -> ApiResult<Reservation> {
    let reservation = state
        .store()
        .reservation(reservation_id)
        .await?
        .ok_or_else(|| ApiError::not_found("reservation"))?;
    let sale = state
        .store()
        .sale(reservation.sale_id)
        .await?
        .ok_or_else(|| ApiError::not_found("sale"))?;
    if sale.accepts(attestor.id) {
        Ok(reservation)
    } else {
        Err(ApiError::not_found("reservation"))
    }
}

/// Records what an attestor reports: a payment, or a refund Kippu asked for. Safe to retry:
/// a report is recorded once per `attestation_id`, and every delivery gets the same answer.
#[tracing::instrument(skip_all)]
pub async fn record_attestation(
    state: &AppState,
    attestor: &Attestor,
    report: AttestorReport,
) -> ApiResult<PaymentResult> {
    match report {
        AttestorReport::Paid(payment) => settle(state, attestor, payment).await,
        AttestorReport::Refunded {
            attestation_id,
            refund_id: None,
        } => confirm_refund(state, attestor.id, &attestation_id).await,
        AttestorReport::Refunded {
            attestation_id,
            refund_id: Some(refund_id),
        } => confirm_ticket_refund(state, attestor.id, &attestation_id, refund_id).await,
        AttestorReport::Reversed { attestation_id } => {
            reverse(state, attestor.id, &attestation_id).await
        }
    }
}

/// Records a payment an organizer took in person, through the built-in `manual` attestor.
#[tracing::instrument(skip_all)]
pub async fn manual_payment(
    state: &AppState,
    principal: &Principal,
    reservation_id: ReservationId,
    payment: ManualPayment,
) -> ApiResult<PaymentResult> {
    let reservation = state
        .store()
        .reservation(reservation_id)
        .await?
        .ok_or_else(|| ApiError::not_found("reservation"))?;
    let event = state
        .store()
        .event(reservation.event_id)
        .await?
        .ok_or_else(|| ApiError::not_found("event"))?;
    state.authorize(
        principal,
        PAYMENTS_MANUAL,
        Scope::Organization(event.organization_id),
    )?;
    let manual = state
        .store()
        .attestor(AttestorId::MANUAL)
        .await?
        .ok_or_else(|| ApiError::internal("the built-in manual attestor is missing"))?;
    let payment = IncomingPayment {
        attestation_id: payment.attestation_id,
        reservation_id,
        amount: payment.amount,
        occurred_at: None,
    };
    let result = settle(state, &manual, payment).await?;
    state
        .audit(principal, "payment.manual", reservation_id)
        .await?;
    Ok(result)
}

/// Records an attested payment and settles the reservation it pays for. Idempotent: the same
/// `(attestor, attestation_id)` delivered any number of times settles once and always yields
/// the same answer.
#[tracing::instrument(skip_all)]
pub async fn settle(
    state: &AppState,
    attestor: &Attestor,
    payment: IncomingPayment,
) -> ApiResult<PaymentResult> {
    validate_attestation_id(&payment.attestation_id)?;
    let store = state.store();
    if let Some(existing) = store
        .attestation(attestor.id, &payment.attestation_id)
        .await?
    {
        return replay(state, existing).await;
    }

    let reservation = store
        .reservation(payment.reservation_id)
        .await?
        .ok_or_else(|| ApiError::not_found("reservation"))?;
    let sale = store
        .sale(reservation.sale_id)
        .await?
        .ok_or_else(|| ApiError::not_found("sale"))?;
    if !attestor.id.is_builtin() && !sale.accepts(attestor.id) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "attestor-not-accepted",
            "this sale does not accept payments from this attestor",
        ));
    }
    if attestor.environment == Environment::Sandbox && reservation.environment == Environment::Live
    {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "environment-mismatch",
            "a sandbox attestor cannot settle a live sale",
        ));
    }
    if payment.amount != reservation.total {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "amount-mismatch",
            format!(
                "the reservation costs {}, not {}",
                reservation.total, payment.amount
            ),
        ));
    }
    let ticket_types = store.list_ticket_types(reservation.sale_id).await?;

    let now = state.now();
    let attestation = PaymentAttestation {
        attestor_id: attestor.id,
        attestation_id: payment.attestation_id,
        reservation_id: payment.reservation_id,
        amount: payment.amount,
        occurred_at: payment.occurred_at.unwrap_or(now),
        received_at: now,
        disposition: PaymentDisposition::Applied,
    };
    let mut tx = store.begin().await?;
    if let Insertion::Existing(existing) = tx.payments().insert_attestation(&attestation).await? {
        drop(tx);
        return replay(state, existing).await;
    }
    let reservation = tx
        .reservations()
        .lock_reservation(payment.reservation_id)
        .await?
        .ok_or_else(|| ApiError::not_found("reservation"))?;

    let items = line_items(&reservation);
    let view = match reservation.status.settle() {
        Settlement::Issue => {
            tx.inventory().sell_held(&items).await?;
            issue(state, &mut *tx, reservation, &ticket_types, now).await?
        }
        Settlement::Reacquire => {
            let account = reservation.account_id;
            let quota = tx
                .inventory()
                .try_take_quota(account, &holds(&items, &ticket_types)?)
                .await?;
            if quota && tx.inventory().try_sell(&items).await? {
                issue(state, &mut *tx, reservation, &ticket_types, now).await?
            } else {
                if quota {
                    tx.inventory().return_quota(account, &items).await?;
                }
                let mut reservation = reservation;
                reservation.status = reservation.status.require_refund()?;
                reservation.updated_at = now;
                tx.reservations().update_reservation(&reservation).await?;
                require_refund(&mut *tx, &attestation, reservation, now).await?
            }
        }
        Settlement::RefundPayment => {
            require_refund(&mut *tx, &attestation, reservation, now).await?
        }
    };
    tx.commit().await?;
    log_settlement(attestor, &view);
    Ok(view)
}

fn log_settlement(attestor: &Attestor, view: &PaymentResult) {
    tracing::info!(
        reservation = %view.reservation.id,
        attestor = %attestor.id,
        disposition = ?view.disposition,
        tickets = view.ticket_ids.len(),
        "payment recorded"
    );
}

async fn issue(
    state: &AppState,
    tx: &mut dyn StoreTx,
    mut reservation: Reservation,
    ticket_types: &[TicketType],
    now: Timestamp,
) -> ApiResult<PaymentResult> {
    let tickets = issue_tickets(state, &reservation, ticket_types, now)?;
    tx.tickets().insert_tickets(&tickets).await?;
    reservation.status = reservation.status.issue()?;
    reservation.updated_at = now;
    tx.reservations().update_reservation(&reservation).await?;
    let ticket_ids: Vec<TicketId> = tickets.iter().map(|ticket| ticket.id).collect();
    let event = IntegrationEvent::TicketsIssued {
        reservation_id: reservation.id,
        account_id: reservation.account_id,
        ticket_ids: ticket_ids.clone(),
    };
    tx.outbox().append_event(&event, now).await?;
    Ok(PaymentResult {
        disposition: PaymentDisposition::Applied,
        reservation,
        ticket_ids,
        replayed: false,
        refund: None,
    })
}

async fn require_refund(
    tx: &mut dyn StoreTx,
    attestation: &PaymentAttestation,
    reservation: Reservation,
    now: Timestamp,
) -> ApiResult<PaymentResult> {
    tx.payments()
        .set_disposition(
            attestation.attestor_id,
            &attestation.attestation_id,
            PaymentDisposition::RefundRequired,
        )
        .await?;
    let event = IntegrationEvent::RefundRequired {
        reservation_id: reservation.id,
        attestor_id: attestation.attestor_id,
        attestation_id: attestation.attestation_id.clone(),
        amount: attestation.amount,
        refund_id: None,
    };
    tx.outbox().append_event(&event, now).await?;
    Ok(PaymentResult {
        disposition: PaymentDisposition::RefundRequired,
        reservation,
        ticket_ids: Vec::new(),
        replayed: false,
        refund: None,
    })
}

/// The answer to an attestation that was already recorded.
async fn replay(state: &AppState, attestation: PaymentAttestation) -> ApiResult<PaymentResult> {
    let reservation = state
        .store()
        .reservation(attestation.reservation_id)
        .await?
        .ok_or_else(|| ApiError::not_found("reservation"))?;
    let ticket_ids = if attestation.disposition == PaymentDisposition::Applied {
        let tickets = state
            .store()
            .tickets_for_reservation(reservation.id)
            .await?;
        tickets.into_iter().map(|ticket| ticket.id).collect()
    } else {
        Vec::new()
    };
    Ok(PaymentResult {
        disposition: attestation.disposition,
        reservation,
        ticket_ids,
        replayed: true,
        refund: None,
    })
}

/// The attestor confirms it returned a payment that required a refund. Idempotent.
#[tracing::instrument(skip_all)]
pub async fn confirm_refund(
    state: &AppState,
    attestor: AttestorId,
    attestation_id: &str,
) -> ApiResult<PaymentResult> {
    let store = state.store();
    let attestation = store
        .attestation(attestor, attestation_id)
        .await?
        .ok_or_else(|| ApiError::not_found("attestation"))?;
    match attestation.disposition {
        PaymentDisposition::Refunded => return replay(state, attestation).await,
        PaymentDisposition::Applied => {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "payment-applied",
                "this payment paid for issued tickets: confirm a refund of them with its \
                 refund_id, or report a reversal",
            ));
        }
        PaymentDisposition::RefundRequired => {}
    }
    let now = state.now();
    let mut tx = store.begin().await?;
    tx.payments()
        .set_disposition(attestor, attestation_id, PaymentDisposition::Refunded)
        .await?;
    let mut reservation = tx
        .reservations()
        .lock_reservation(attestation.reservation_id)
        .await?
        .ok_or_else(|| ApiError::not_found("reservation"))?;
    if let Ok(refunded) = reservation.status.refunded() {
        reservation.status = refunded;
        reservation.updated_at = now;
        tx.reservations().update_reservation(&reservation).await?;
    }
    tx.commit().await?;
    Ok(PaymentResult {
        disposition: PaymentDisposition::Refunded,
        reservation,
        ticket_ids: Vec::new(),
        replayed: false,
        refund: None,
    })
}

/// The answer to a report about a refund of issued tickets.
async fn refund_result(
    state: &AppState,
    attestation: PaymentAttestation,
    refund: Refund,
    replayed: bool,
) -> ApiResult<PaymentResult> {
    let mut result = replay(state, attestation).await?;
    result.replayed = replayed;
    result.refund = Some(refund);
    Ok(result)
}

/// The attestor confirms it returned the money of a refund of issued tickets. Idempotent.
#[tracing::instrument(skip_all)]
pub async fn confirm_ticket_refund(
    state: &AppState,
    attestor: AttestorId,
    attestation_id: &str,
    refund_id: RefundId,
) -> ApiResult<PaymentResult> {
    let store = state.store();
    let attestation = store
        .attestation(attestor, attestation_id)
        .await?
        .ok_or_else(|| ApiError::not_found("attestation"))?;
    let refund = store
        .refund(refund_id)
        .await?
        .filter(|refund| refund.attestor_id == attestor && refund.attestation_id == attestation_id)
        .ok_or_else(|| ApiError::not_found("refund"))?;
    if refund.status == RefundStatus::Completed {
        return refund_result(state, attestation, refund, true).await;
    }
    let now = state.now();
    let mut tx = store.begin().await?;
    let completed = tx.payments().complete_refund(refund.id, now).await?;
    tx.commit().await?;
    if completed {
        tracing::info!(refund = %refund.id, attestor = %attestor, "refund confirmed");
    }
    let refund = store
        .refund(refund.id)
        .await?
        .ok_or_else(|| ApiError::not_found("refund"))?;
    refund_result(state, attestation, refund, !completed).await
}

/// The attestor reports that a payment's money went back as a whole without Kippu asking: a
/// chargeback, or a full refund made in the provider's dashboard. The tickets it still paid for
/// are revoked and their stock returned, and the reversal is recorded as a completed refund.
/// Refunds of the payment still pending are completed too: their money went back with it.
/// Idempotent.
#[tracing::instrument(skip_all)]
pub async fn reverse(
    state: &AppState,
    attestor: AttestorId,
    attestation_id: &str,
) -> ApiResult<PaymentResult> {
    let store = state.store();
    let attestation = store
        .attestation(attestor, attestation_id)
        .await?
        .ok_or_else(|| ApiError::not_found("attestation"))?;
    if attestation.disposition != PaymentDisposition::Applied {
        // It paid for no tickets: the money owed back is back.
        return confirm_refund(state, attestor, attestation_id).await;
    }
    let id = RefundId::reversal(attestor, attestation_id);
    if let Some(existing) = store.refund(id).await? {
        return refund_result(state, attestation, existing, true).await;
    }
    // Read before the transaction. A refund racing this one revokes some of these tickets
    // first, and `revoke` then fails with 409: the attestor retries and reads afresh.
    let tickets: Vec<_> = store
        .tickets_for_reservation(attestation.reservation_id)
        .await?
        .into_iter()
        .filter(|ticket| ticket.status == TicketStatus::Valid)
        .collect();
    let pending: Vec<RefundId> = store
        .refunds_for_reservation(attestation.reservation_id)
        .await?
        .into_iter()
        .filter(|refund| {
            refund.status == RefundStatus::Pending
                && refund.attestor_id == attestor
                && refund.attestation_id == attestation_id
        })
        .map(|refund| refund.id)
        .collect();
    let now = state.now();
    let mut tx = store.begin().await?;
    let reservation = tx
        .reservations()
        .lock_reservation(attestation.reservation_id)
        .await?
        .ok_or_else(|| ApiError::not_found("reservation"))?;
    let refund = new_refund(
        id,
        &reservation,
        &attestation,
        &tickets,
        RefundReason::Reversal,
        now,
    )?;
    if let Insertion::Existing(existing) =
        revoke(&mut *tx, &reservation, &refund, &tickets, now).await?
    {
        drop(tx);
        return refund_result(state, attestation, existing, true).await;
    }
    for pending in &pending {
        tx.payments().complete_refund(*pending, now).await?;
    }
    tx.commit().await?;
    tracing::info!(
        refund = %refund.id,
        reservation = %reservation.id,
        attestor = %attestor,
        tickets = refund.ticket_ids.len(),
        pending_refunds_completed = pending.len(),
        "payment reversed; tickets revoked"
    );
    refund_result(state, attestation, refund, false).await
}
