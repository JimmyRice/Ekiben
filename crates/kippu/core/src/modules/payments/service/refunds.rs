//! Refunds of issued tickets.
//!
//! A refund is asked of Kippu — by the buyer, within the ticket type's refund period, or by an
//! organizer at any time — never of the attestor. In one transaction the tickets are revoked,
//! their stock returns to the pool, the buyer's quota is given back and a `refund.required`
//! event (with the refund's id) tells the attestor to return the money. The tickets are dead
//! from that moment, whatever happens to the money: revoking first means the worst case is a
//! refund that arrives late, never money returned for a ticket that still works.
//!
//! The attestor confirms with `outcome: refunded` and the refund's id. Free tickets have no
//! money to return and complete at once; cash taken in person is confirmed by an organizer. A
//! partial refund made outside Kippu has no other way in: it is asked of Kippu like any other.

use std::collections::BTreeSet;

use kippu_domain::catalog::Event;
use kippu_domain::outbox::IntegrationEvent;
use kippu_domain::payment::{PaymentAttestation, PaymentDisposition};
use kippu_domain::refund::{Refund, RefundReason, RefundStatus};
use kippu_domain::reservation::{Reservation, ReservationStatus};
use kippu_domain::ticket::{Ticket, TicketStatus};
use kippu_domain::{AttestorId, Money, RefundId, ReservationId, TicketId, Timestamp};
use kippu_store::{Insertion, LineItems, StoreTx};

use crate::app::AppState;
use crate::auth::{Principal, Scope};
use crate::error::{ApiError, ApiResult, StatusCode};
use crate::modules::payments::permissions::{PAYMENTS_MANUAL, REFUNDS_MANAGE, REFUNDS_REQUEST};

/// What to refund.
#[derive(Debug, Clone, Default)]
pub struct RefundRequest {
    /// The tickets to refund; empty means every ticket of the reservation still valid.
    pub ticket_ids: Vec<TicketId>,
}

/// How the caller may act on a reservation's refunds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Acting {
    /// An organizer of the event (or an admin): no refund period applies.
    Organizer,
    /// The buyer: only within the ticket types' refund periods.
    Buyer,
}

/// How `principal` may act on refunds of `reservation`, an order for `event`; `None` if not at
/// all.
fn acting(
    state: &AppState,
    principal: &Principal,
    reservation: &Reservation,
    event: &Event,
) -> Option<Acting> {
    let policy = state.policy();
    if policy.permits(
        principal,
        REFUNDS_MANAGE,
        Scope::Organization(event.organization_id),
    ) {
        Some(Acting::Organizer)
    } else if policy.permits(
        principal,
        REFUNDS_REQUEST,
        Scope::Account(reservation.account_id),
    ) {
        Some(Acting::Buyer)
    } else {
        None
    }
}

/// The reservation and its event, and how the caller may act on them. Reservations the caller
/// may not refund look missing (`what` names the record reported missing).
async fn refundable_reservation(
    state: &AppState,
    principal: &Principal,
    id: ReservationId,
    what: &'static str,
) -> ApiResult<(Reservation, Event, Acting)> {
    let reservation = state
        .store()
        .reservation(id)
        .await?
        .ok_or_else(|| ApiError::not_found(what))?;
    let event = state
        .store()
        .event(reservation.event_id)
        .await?
        .ok_or_else(|| ApiError::not_found(what))?;
    let acting =
        acting(state, principal, &reservation, &event).ok_or_else(|| ApiError::not_found(what))?;
    Ok((reservation, event, acting))
}

/// The payment that paid for a reservation's tickets.
async fn applied_payment(
    state: &AppState,
    reservation: ReservationId,
) -> ApiResult<PaymentAttestation> {
    state
        .store()
        .attestations_for_reservation(reservation)
        .await?
        .into_iter()
        .find(|attestation| attestation.disposition == PaymentDisposition::Applied)
        .ok_or_else(|| ApiError::internal("an issued reservation has no applied payment"))
}

/// The tickets to refund: those named, or every valid one.
fn chosen_tickets(tickets: Vec<Ticket>, named: &[TicketId]) -> ApiResult<Vec<Ticket>> {
    if named.is_empty() {
        let valid: Vec<Ticket> = tickets
            .into_iter()
            .filter(|ticket| ticket.status == TicketStatus::Valid)
            .collect();
        if valid.is_empty() {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "nothing-to-refund",
                "every ticket of this reservation is already refunded",
            ));
        }
        return Ok(valid);
    }
    let named: BTreeSet<TicketId> = named.iter().copied().collect();
    let chosen: Vec<Ticket> = tickets
        .into_iter()
        .filter(|ticket| named.contains(&ticket.id))
        .collect();
    if chosen.len() != named.len() {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "ticket-not-in-reservation",
            "a ticket to refund is not one of this reservation's",
        ));
    }
    if chosen
        .iter()
        .any(|ticket| ticket.status != TicketStatus::Valid)
    {
        return Err(already_revoked());
    }
    Ok(chosen)
}

#[track_caller]
fn already_revoked() -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        "ticket-revoked",
        "a ticket to refund is already revoked",
    )
}

/// Checks that the buyer may still refund `tickets` themselves: their types are refundable,
/// the refund period has not ended, and neither the tickets nor the event have started.
async fn check_refund_period(
    state: &AppState,
    reservation: &Reservation,
    event: &Event,
    tickets: &[Ticket],
    now: Timestamp,
) -> ApiResult<()> {
    let ticket_types = state.store().list_ticket_types(reservation.sale_id).await?;
    for ticket in tickets {
        let until = ticket_types
            .iter()
            .find(|ticket_type| ticket_type.id == ticket.ticket_type_id)
            .and_then(|ticket_type| ticket_type.refundable_until)
            .ok_or_else(|| {
                ApiError::new(
                    StatusCode::CONFLICT,
                    "refund-not-allowed",
                    "these tickets cannot be refunded; ask the organizer",
                )
            })?;
        if now >= until || now >= ticket.valid_from || now >= event.starts_at {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "refund-not-allowed",
                "the refund period for these tickets has ended; ask the organizer",
            ));
        }
    }
    Ok(())
}

/// What the tickets were bought for: the reservation's price of each ticket's type.
fn price_of(reservation: &Reservation, tickets: &[Ticket]) -> ApiResult<Money> {
    let mut amount = Money::zero(reservation.total.currency());
    for ticket in tickets {
        let unit_price = reservation
            .items
            .iter()
            .find(|item| item.ticket_type_id == ticket.ticket_type_id)
            .map(|item| item.unit_price)
            .ok_or_else(|| ApiError::internal("a ticket's type is not in its reservation"))?;
        amount = amount.try_add(unit_price).map_err(ApiError::internal)?;
    }
    Ok(amount)
}

/// A new refund of `tickets`, paid for by `payment`.
pub(super) fn new_refund(
    id: RefundId,
    reservation: &Reservation,
    payment: &PaymentAttestation,
    tickets: &[Ticket],
    reason: RefundReason,
    now: Timestamp,
) -> ApiResult<Refund> {
    let amount = price_of(reservation, tickets)?;
    // Nothing to return, or money that is already gone: complete from the start.
    let completed = amount.is_zero() || reason == RefundReason::Reversal;
    let mut ticket_ids: Vec<TicketId> = tickets.iter().map(|ticket| ticket.id).collect();
    ticket_ids.sort();
    Ok(Refund {
        id,
        reservation_id: reservation.id,
        account_id: reservation.account_id,
        event_id: reservation.event_id,
        attestor_id: payment.attestor_id,
        attestation_id: payment.attestation_id.clone(),
        ticket_ids,
        amount,
        reason,
        status: if completed {
            RefundStatus::Completed
        } else {
            RefundStatus::Pending
        },
        created_at: now,
        completed_at: completed.then_some(now),
    })
}

/// Revokes the refund's tickets, returns their stock and quota, and records the refund and
/// its events, inside `tx`. The reservation must be locked by `tx`.
///
/// Returns the refund already recorded under the same id instead, changing nothing, if there
/// is one. Fails with `409 ticket-revoked` if a ticket was revoked meanwhile.
pub(super) async fn revoke(
    tx: &mut dyn StoreTx,
    reservation: &Reservation,
    refund: &Refund,
    tickets: &[Ticket],
    now: Timestamp,
) -> ApiResult<Insertion<Refund>> {
    if let Insertion::Existing(existing) = tx.payments().insert_refund(refund).await? {
        return Ok(Insertion::Existing(existing));
    }
    let revoked = tx.tickets().revoke_tickets(&refund.ticket_ids).await?;
    if revoked != u64::try_from(refund.ticket_ids.len()).unwrap_or(u64::MAX) {
        return Err(already_revoked());
    }
    let items = LineItems::count(tickets.iter().map(|ticket| ticket.ticket_type_id));
    // Quota before stock: the order purchases and late payments take them in, so a refund
    // cannot deadlock with the same buyer's purchase.
    tx.inventory()
        .return_quota(reservation.account_id, &items)
        .await?;
    tx.inventory().return_sold(&items).await?;
    // A reversal of a payment whose tickets were all refunded already revokes nothing.
    if !refund.ticket_ids.is_empty() {
        let revoked = IntegrationEvent::TicketsRevoked {
            reservation_id: reservation.id,
            event_id: reservation.event_id,
            ticket_ids: refund.ticket_ids.clone(),
            reason: refund.reason,
        };
        tx.outbox().append_event(&revoked, now).await?;
    }
    if refund.status == RefundStatus::Pending {
        let owed = IntegrationEvent::RefundRequired {
            reservation_id: reservation.id,
            attestor_id: refund.attestor_id,
            attestation_id: refund.attestation_id.clone(),
            amount: refund.amount,
            refund_id: Some(refund.id),
        };
        tx.outbox().append_event(&owed, now).await?;
    }
    Ok(Insertion::Inserted)
}

/// Refunds tickets of a reservation: revokes them at once and asks the attestor to return the
/// money. The buyer may do so within the refund period of the tickets' types; an organizer of
/// the event at any time.
#[tracing::instrument(skip_all)]
pub async fn request_refund(
    state: &AppState,
    principal: &Principal,
    reservation_id: ReservationId,
    request: RefundRequest,
) -> ApiResult<Refund> {
    let (reservation, event, acting) =
        refundable_reservation(state, principal, reservation_id, "reservation").await?;
    if reservation.status != ReservationStatus::Issued {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "reservation-not-issued",
            "only a reservation whose tickets were issued can be refunded",
        ));
    }
    let tickets = state
        .store()
        .tickets_for_reservation(reservation.id)
        .await?;
    let tickets = chosen_tickets(tickets, &request.ticket_ids)?;
    let now = state.now();
    if acting == Acting::Buyer {
        check_refund_period(state, &reservation, &event, &tickets, now).await?;
    }
    let payment = applied_payment(state, reservation.id).await?;
    let refund = new_refund(
        RefundId::generate(),
        &reservation,
        &payment,
        &tickets,
        RefundReason::Requested,
        now,
    )?;

    let mut tx = state.store().begin().await?;
    let reservation = tx
        .reservations()
        .lock_reservation(reservation.id)
        .await?
        .ok_or_else(|| ApiError::not_found("reservation"))?;
    if let Insertion::Existing(_) = revoke(&mut *tx, &reservation, &refund, &tickets, now).await? {
        return Err(ApiError::internal("a new refund id is already taken"));
    }
    tx.commit().await?;
    state
        .audit(principal, "refund.request", reservation.id)
        .await?;
    tracing::info!(
        refund = %refund.id,
        reservation = %reservation.id,
        tickets = refund.ticket_ids.len(),
        status = refund.status.as_str(),
        "tickets refunded"
    );
    Ok(refund)
}

/// The refunds of a reservation, oldest first, for its buyer and the event's organizers.
#[tracing::instrument(skip_all)]
pub async fn reservation_refunds(
    state: &AppState,
    principal: &Principal,
    reservation_id: ReservationId,
) -> ApiResult<Vec<Refund>> {
    let (reservation, _, _) =
        refundable_reservation(state, principal, reservation_id, "reservation").await?;
    Ok(state
        .store()
        .refunds_for_reservation(reservation.id)
        .await?)
}

/// A refund, for its buyer and the event's organizers. Others' refunds look missing.
#[tracing::instrument(skip_all)]
pub async fn refund(state: &AppState, principal: &Principal, id: RefundId) -> ApiResult<Refund> {
    let refund = state
        .store()
        .refund(id)
        .await?
        .ok_or_else(|| ApiError::not_found("refund"))?;
    refundable_reservation(state, principal, refund.reservation_id, "refund").await?;
    Ok(refund)
}

/// An organizer confirms that cash taken in person (the built-in `manual` attestor) was
/// handed back. Idempotent.
#[tracing::instrument(skip_all)]
pub async fn confirm_manual_refund(
    state: &AppState,
    principal: &Principal,
    id: RefundId,
) -> ApiResult<Refund> {
    let refund = refund(state, principal, id).await?;
    let event = state
        .store()
        .event(refund.event_id)
        .await?
        .ok_or_else(|| ApiError::not_found("refund"))?;
    state.authorize(
        principal,
        PAYMENTS_MANUAL,
        Scope::Organization(event.organization_id),
    )?;
    if refund.attestor_id != AttestorId::MANUAL {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "refund-not-manual",
            "only refunds of payments taken in person are confirmed by organizers",
        ));
    }
    if refund.status == RefundStatus::Completed {
        return Ok(refund);
    }
    let now = state.now();
    let mut tx = state.store().begin().await?;
    let completed = tx.payments().complete_refund(refund.id, now).await?;
    tx.commit().await?;
    if completed {
        state
            .audit(principal, "refund.confirm-manual", refund.id)
            .await?;
        tracing::info!(refund = %refund.id, "cash refund confirmed");
    }
    state
        .store()
        .refund(refund.id)
        .await?
        .ok_or_else(|| ApiError::not_found("refund"))
}
