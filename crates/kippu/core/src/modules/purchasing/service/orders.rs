//! What organizers read: the reservations of their events, whoever bought them, and how to
//! reach those people.

use kippu_domain::account::Account;
use kippu_domain::reservation::{Reservation, ReservationStatus};
use kippu_domain::{AccountId, EventId};
use kippu_store::{Keyset, Page, PageRequest};

use crate::app::AppState;
use crate::auth::{Principal, Scope};
use crate::error::{ApiError, ApiResult};
use crate::modules::catalog::service::visible_event;
use crate::modules::purchasing::permissions::ORDERS_READ;

/// Whether `principal` may read the orders of `event` — its reservations and tickets, whoever
/// bought them. The event is read only for callers whose role could.
pub async fn reads_orders_of(
    state: &AppState,
    principal: &Principal,
    event: EventId,
) -> ApiResult<bool> {
    let policy = state.policy();
    if !policy.allows(principal.role(), ORDERS_READ) {
        return Ok(false);
    }
    let Some(event) = state.store().event_summary(event).await? else {
        return Ok(false);
    };
    Ok(policy.permits(
        principal,
        ORDERS_READ,
        Scope::Organization(event.organization_id),
    ))
}

/// An event's reservations, all of them or those in `status`, newest first, for its
/// organizers.
#[tracing::instrument(skip_all)]
pub async fn event_reservations(
    state: &AppState,
    principal: &Principal,
    event_id: EventId,
    status: Option<ReservationStatus>,
    page: PageRequest<Keyset>,
) -> ApiResult<Page<Reservation, Keyset>> {
    let event = visible_event(state, Some(principal), event_id).await?;
    state.authorize(
        principal,
        ORDERS_READ,
        Scope::Organization(event.organization_id),
    )?;
    let reservations = state
        .store()
        .reservations_for_event(event.id, status, page.plus_one())
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

/// Someone who ordered for an event (a reservation that has not expired or been cancelled),
/// for the event's organizers: to reach them about the event, through the contact details
/// they share. Others are reported as missing.
#[tracing::instrument(skip_all)]
pub async fn buyer(
    state: &AppState,
    principal: &Principal,
    event_id: EventId,
    account_id: AccountId,
) -> ApiResult<Account> {
    let event = visible_event(state, Some(principal), event_id).await?;
    state.authorize(
        principal,
        ORDERS_READ,
        Scope::Organization(event.organization_id),
    )?;
    if !state.store().has_orders(event.id, Some(account_id)).await? {
        return Err(ApiError::not_found("buyer"));
    }
    state
        .store()
        .account(account_id)
        .await?
        .ok_or_else(|| ApiError::not_found("buyer"))
}
