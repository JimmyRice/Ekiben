//! Buyers' favourite events.

use kippu_domain::catalog::EventSummary;
use kippu_domain::{AccountId, EventId};
use kippu_store::{Favorite, Keyset, Page, PageRequest};

use super::visible_event;
use crate::app::AppState;
use crate::auth::{Principal, Scope};
use crate::error::ApiResult;
use crate::modules::catalog::permissions::FAVORITES_MANAGE;

/// The caller's own account, after checking they may keep favourites.
fn own_account(state: &AppState, principal: &Principal) -> ApiResult<AccountId> {
    let account = principal.require_account()?;
    state.authorize(principal, FAVORITES_MANAGE, Scope::Account(account))?;
    Ok(account)
}

/// The caller's favourite events that they can still see, most recently marked first. A page
/// leaves out events that have since become hidden, so it may hold fewer than `page.limit`.
#[tracing::instrument(skip_all)]
pub async fn favorites(
    state: &AppState,
    principal: &Principal,
    page: PageRequest<Keyset>,
) -> ApiResult<Page<EventSummary, Keyset>> {
    let account = own_account(state, principal)?;
    let favorites = state.store().favorites(account, page.plus_one()).await?;
    let favorites = Page::from_lookahead(favorites, page.limit, Favorite::position);
    let mut events = Vec::new();
    for favorite in favorites.items {
        if let Ok(event) = visible_event(state, Some(principal), favorite.event_id).await {
            events.push(event);
        }
    }
    Ok(Page {
        items: events,
        next: favorites.next,
    })
}

/// Marks an event the caller can see as a favourite.
#[tracing::instrument(skip_all)]
pub async fn add_favorite(
    state: &AppState,
    principal: &Principal,
    event_id: EventId,
) -> ApiResult<()> {
    let account = own_account(state, principal)?;
    visible_event(state, Some(principal), event_id).await?;
    state
        .store()
        .add_favorite(account, event_id, state.now())
        .await?;
    Ok(())
}

/// Forgets a favourite.
#[tracing::instrument(skip_all)]
pub async fn remove_favorite(
    state: &AppState,
    principal: &Principal,
    event_id: EventId,
) -> ApiResult<()> {
    let account = own_account(state, principal)?;
    state.store().remove_favorite(account, event_id).await?;
    Ok(())
}
