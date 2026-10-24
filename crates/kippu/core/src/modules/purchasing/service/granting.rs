//! Grants: organizers give a private sale's free tickets to the accounts they choose.
//!
//! A grant is recorded like a purchase decided on the spot: a purchase request, already
//! reserved, and its reservation, written in one transaction that takes the account's quota
//! and the stock (in that order, as every transaction does). The reservation is then settled
//! like any free one, so its tickets, their `tickets.issued` event, refunds and denials work
//! as for tickets bought. Nobody buys from a private sale, so no purchase worker serves it.

use kippu_domain::catalog::{Audience, EventSummary, Sale, TicketType};
use kippu_domain::purchase::{Basket, LineItem, PurchaseRequest, PurchaseStatus};
use kippu_domain::reservation::{Reservation, ReservationStatus};
use kippu_domain::validation::IdempotencyKey;
use kippu_domain::{
    AccountId, Duration, PurchaseRequestId, ReservationId, SaleId, ValidationError,
};
use kippu_store::{Insertion, LineItems};

use super::buying::settle_free;
use super::pipeline::reserved_items;
use crate::app::AppState;
use crate::auth::{Principal, Scope};
use crate::error::{ApiError, ApiResult, ProblemKind, StatusCode};
use crate::modules::catalog::service::visible_sale;
use crate::modules::payments::service::holds;
use crate::modules::purchasing::permissions::TICKETS_GRANT;

/// Tickets an organizer grants an account.
#[derive(Debug, Clone)]
pub struct Grant {
    /// The organizer's key for this grant: granting again with it never grants twice. Required.
    pub idempotency_key: Option<String>,
    /// Who gets the tickets.
    pub account_id: AccountId,
    /// Ticket types and quantities.
    pub items: Vec<LineItem>,
}

/// A grant, and whether this call made it: granting again with the same key returns the
/// first grant's reservation as it now stands.
#[derive(Debug, Clone)]
pub struct Granted {
    /// The reservation holding the granted tickets, `issued`.
    pub reservation: Reservation,
    /// Whether this call made the grant.
    pub created: bool,
}

/// Grants an account free tickets of a private sale, issued at once, while the sale is open
/// and its event published. The account's per-type limits and the stock apply as to a
/// purchase; deny lists too. Granting again with the same key changes nothing.
#[tracing::instrument(skip_all)]
pub async fn grant_tickets(
    state: &AppState,
    principal: &Principal,
    sale_id: SaleId,
    grant: Grant,
) -> ApiResult<Granted> {
    let key = grant
        .idempotency_key
        .ok_or(ValidationError::new(
            "Idempotency-Key",
            "header is required",
        ))
        .and_then(IdempotencyKey::new)?;
    let (sale, event) = grantable_sale(state, principal, sale_id).await?;
    let basket = Basket::new(grant.items)?;
    let ticket_types = state.store().list_ticket_types(sale.id).await?;
    check_basket(&sale, &ticket_types, &basket)?;
    let id = PurchaseRequestId::derive(grant.account_id, sale.id, &key);
    if let Some(existing) = state.store().purchase_request(id).await? {
        return granted_before(state, existing, &basket).await;
    }
    check_recipient(state, &event, grant.account_id).await?;
    let now = state.now();
    if !event.is_on_sale() || !sale.is_open_at(now) {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            ProblemKind::SALE_CLOSED,
            "tickets are granted only while the sale is open and its event published",
        ));
    }

    let request = PurchaseRequest {
        id,
        account_id: grant.account_id,
        sale_id: sale.id,
        basket,
        status: PurchaseStatus::Reserved {
            reservation_id: ReservationId::generate(),
        },
        created_at: now,
        updated_at: now,
    };
    let reservation = grant_reservation(&request, &sale, &ticket_types)?;
    if let Some(existing) = record_grant(state, &request, &reservation, &ticket_types).await? {
        // The same grant, made at the same moment.
        return granted_before(state, existing, &request.basket).await;
    }
    state
        .audit(principal, "tickets.grant", reservation.id)
        .await?;
    tracing::info!(
        reservation = %reservation.id,
        account = %reservation.account_id,
        "tickets granted"
    );
    // Should this fail, granting again with the key settles the reservation.
    let reservation = settle_free(state, &reservation).await?;
    Ok(Granted {
        reservation,
        created: true,
    })
}

/// A private sale whose tickets the caller may grant. Others' private sales look missing.
async fn grantable_sale(
    state: &AppState,
    principal: &Principal,
    id: SaleId,
) -> ApiResult<(Sale, EventSummary)> {
    let (sale, event) = visible_sale(state, Some(principal), id).await?;
    state.authorize(
        principal,
        TICKETS_GRANT,
        Scope::Organization(event.organization_id),
    )?;
    if sale.audience != Audience::Private {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            ProblemKind::WRONG_SALE_AUDIENCE,
            "only a private sale's tickets are granted: invite the account to buy instead",
        ));
    }
    Ok((sale, event))
}

/// Checks that a grant names only the sale's ticket types, and no more tickets than one
/// request may ask for.
fn check_basket(sale: &Sale, ticket_types: &[TicketType], basket: &Basket) -> ApiResult<()> {
    if !basket.items().iter().all(|item| {
        ticket_types
            .iter()
            .any(|ticket_type| ticket_type.id == item.ticket_type_id)
    }) {
        return Err(ValidationError::new(
            "items",
            "name a ticket type that is not part of the sale",
        )
        .into());
    }
    if basket.total_quantity() > u64::from(sale.max_tickets_per_request) {
        return Err(ValidationError::new(
            "items",
            "ask for more tickets than the sale's max_tickets_per_request",
        )
        .into());
    }
    Ok(())
}

/// The reservation a grant's request makes: free, and held until it is settled.
fn grant_reservation(
    request: &PurchaseRequest,
    sale: &Sale,
    ticket_types: &[TicketType],
) -> ApiResult<Reservation> {
    let PurchaseStatus::Reserved { reservation_id } = request.status else {
        return Err(ApiError::internal("a grant's request is not reserved"));
    };
    let (items, total) = reserved_items(request, ticket_types)?;
    if !total.is_zero() {
        return Err(ApiError::internal(
            "a private sale's ticket type has a price",
        ));
    }
    let now = request.created_at;
    Ok(Reservation {
        id: reservation_id,
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
    })
}

/// Records a grant's request and reservation in one transaction, taking the account's quota
/// and then the stock. Writes nothing, and returns the request, if one with its id exists.
async fn record_grant(
    state: &AppState,
    request: &PurchaseRequest,
    reservation: &Reservation,
    ticket_types: &[TicketType],
) -> ApiResult<Option<PurchaseRequest>> {
    let lines = LineItems::from(&request.basket);
    let quota = holds(&lines, ticket_types)?;
    let mut tx = state.store().begin().await?;
    if let Insertion::Existing(existing) = tx.purchases().insert_purchase_request(request).await? {
        return Ok(Some(existing));
    }
    if !tx
        .inventory()
        .try_take_quota(request.account_id, &quota)
        .await?
    {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            ProblemKind::LIMIT_EXCEEDED,
            "the account would hold more of these tickets than one account may",
        ));
    }
    if !tx.inventory().try_hold(&lines).await? {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            ProblemKind::SOLD_OUT,
            "not enough tickets are left",
        ));
    }
    tx.reservations().insert_reservation(reservation).await?;
    tx.commit().await?;
    Ok(None)
}

/// Checks that the account can receive tickets of `event`: it exists, is not closed, and is
/// not denied entry.
async fn check_recipient(
    state: &AppState,
    event: &EventSummary,
    account: AccountId,
) -> ApiResult<()> {
    let recipient = state
        .store()
        .account(account)
        .await?
        .ok_or(ValidationError::new("account_id", "is not an account"))?;
    if recipient.is_closed() {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            ProblemKind::ACCOUNT_CLOSED,
            "this account is closed",
        ));
    }
    if state
        .store()
        .account_denied(event.organization_id, event.id, account)
        .await?
    {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            ProblemKind::ACCOUNT_DENIED,
            "this account is denied entry to the event",
        ));
    }
    Ok(())
}

/// The answer to granting with a key used before: the same grant again, settled if the first
/// attempt stopped short of that, or a refusal for a different one.
async fn granted_before(
    state: &AppState,
    existing: PurchaseRequest,
    basket: &Basket,
) -> ApiResult<Granted> {
    if existing.basket != *basket {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            ProblemKind::IDEMPOTENCY_KEY_REUSED,
            "this Idempotency-Key was already used for a different grant",
        ));
    }
    let PurchaseStatus::Reserved { reservation_id } = existing.status else {
        return Err(ApiError::internal(
            "a grant's purchase request is not reserved",
        ));
    };
    let reservation = state
        .store()
        .reservation(reservation_id)
        .await?
        .ok_or_else(|| ApiError::internal("a grant's reservation is missing"))?;
    Ok(Granted {
        reservation: settle_free(state, &reservation).await?,
        created: false,
    })
}
