//! Catalog lookups shared by other modules: visibility and write access.

use kippu_domain::catalog::{Event, Sale};
use kippu_domain::{EventId, SaleId};

use super::dto::{SaleDetail, TicketTypeOffer};
use super::permissions::EVENTS_WRITE;
use crate::app::AppState;
use crate::auth::{Principal, Scope};
use crate::error::{ApiError, ApiResult};

/// Whether `principal` may edit (and see drafts of) `event`.
pub(crate) fn can_write(state: &AppState, principal: Option<&Principal>, event: &Event) -> bool {
    principal.is_some_and(|principal| {
        state.policy().permits(
            principal,
            EVENTS_WRITE,
            Scope::Organization(event.organization_id),
        )
    })
}

/// An event the caller may see: published or cancelled, or a draft they could edit.
/// Hidden events are reported as missing rather than forbidden.
pub(crate) async fn visible_event(
    state: &AppState,
    principal: Option<&Principal>,
    id: EventId,
) -> ApiResult<Event> {
    let event = state
        .store()
        .event(id)
        .await?
        .ok_or_else(|| ApiError::not_found("event"))?;
    if event.is_public() || can_write(state, principal, &event) {
        Ok(event)
    } else {
        Err(ApiError::not_found("event"))
    }
}

/// An event the caller may edit.
pub(crate) async fn writable_event(
    state: &AppState,
    principal: &Principal,
    id: EventId,
) -> ApiResult<Event> {
    let event = visible_event(state, Some(principal), id).await?;
    state.authorize(
        principal,
        EVENTS_WRITE,
        Scope::Organization(event.organization_id),
    )?;
    Ok(event)
}

/// A sale whose event the caller may see.
pub(crate) async fn visible_sale(
    state: &AppState,
    principal: Option<&Principal>,
    id: SaleId,
) -> ApiResult<(Sale, Event)> {
    let sale = state
        .store()
        .sale(id)
        .await?
        .ok_or_else(|| ApiError::not_found("sale"))?;
    let event = visible_event(state, principal, sale.event_id).await?;
    Ok((sale, event))
}

/// A sale the caller may edit.
pub(crate) async fn writable_sale(
    state: &AppState,
    principal: &Principal,
    id: SaleId,
) -> ApiResult<(Sale, Event)> {
    let sale = state
        .store()
        .sale(id)
        .await?
        .ok_or_else(|| ApiError::not_found("sale"))?;
    let event = writable_event(state, principal, sale.event_id).await?;
    Ok((sale, event))
}

/// A sale with its ticket types and what is left of each.
pub(crate) async fn sale_detail(state: &AppState, sale: Sale) -> ApiResult<SaleDetail> {
    let mut ticket_types = Vec::new();
    for ticket_type in state.store().list_ticket_types(sale.id).await? {
        let available = state
            .store()
            .inventory(ticket_type.id)
            .await?
            .map_or(0, |inventory| inventory.available());
        ticket_types.push(TicketTypeOffer {
            ticket_type,
            available,
        });
    }
    Ok(SaleDetail { sale, ticket_types })
}
