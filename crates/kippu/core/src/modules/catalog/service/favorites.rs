//! Buyers' favourite events.

use kippu_domain::catalog::EventSummary;
use kippu_domain::{AccountId, EventId};

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

/// The caller's favourite events that they can still see.
#[tracing::instrument(skip_all)]
pub async fn favorites(state: &AppState, principal: &Principal) -> ApiResult<Vec<EventSummary>> {
    let account = own_account(state, principal)?;
    let mut events = Vec::new();
    for event_id in state.store().favorites(account).await? {
        if let Ok(event) = visible_event(state, Some(principal), event_id).await {
            events.push(event.into());
        }
    }
    Ok(events)
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
