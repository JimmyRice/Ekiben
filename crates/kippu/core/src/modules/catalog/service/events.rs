//! Events: creating, editing, publishing, and who may see them.

use kippu_domain::catalog::{Address, Event, EventStatus, EventSummary};
use kippu_domain::validation::Slug;
use kippu_domain::{EventId, OrganizationId, Timestamp};
use kippu_store::{EventFilter, EventOrder, Keyset, Page, PageRequest};

use super::check_version;
use crate::app::AppState;
use crate::auth::{Principal, Scope};
use crate::error::{ApiError, ApiResult};
use crate::modules::catalog::permissions::EVENTS_WRITE;

/// A new event. It starts as a draft.
#[derive(Debug, Clone)]
pub struct NewEvent {
    /// URL-friendly unique name.
    pub slug: String,
    /// Display title.
    pub title: String,
    /// Long description.
    pub description: String,
    /// Where it takes place, as people call it.
    pub venue: String,
    /// The precise address, if there is one.
    pub address: Option<Address>,
    /// When it opens.
    pub starts_at: Timestamp,
    /// When it closes.
    pub ends_at: Timestamp,
    /// The event's page, stored as is.
    pub content: String,
}

/// Changes to an event. `None` keeps the stored value.
#[derive(Debug, Clone, Default)]
pub struct EventChanges {
    /// URL-friendly unique name.
    pub slug: Option<String>,
    /// Display title.
    pub title: Option<String>,
    /// Long description.
    pub description: Option<String>,
    /// Where it takes place, as people call it.
    pub venue: Option<String>,
    /// The precise address; `Some(None)` removes it.
    pub address: Option<Option<Address>>,
    /// When it opens.
    pub starts_at: Option<Timestamp>,
    /// When it closes.
    pub ends_at: Option<Timestamp>,
    /// Publish, cancel or withdraw it.
    pub status: Option<EventStatus>,
    /// The event's page.
    pub content: Option<String>,
}

/// Whether `principal` may edit (and see drafts of) `event`.
pub fn can_write(state: &AppState, principal: Option<&Principal>, event: &Event) -> bool {
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
#[tracing::instrument(skip_all)]
pub async fn visible_event(
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
#[tracing::instrument(skip_all)]
pub async fn writable_event(
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

/// Published and cancelled events, for everyone. `filter`'s organization and visibility are
/// set here.
#[tracing::instrument(skip_all)]
pub async fn public_events(
    state: &AppState,
    filter: EventFilter,
    order: EventOrder,
    page: PageRequest<Keyset>,
) -> ApiResult<Page<EventSummary, Keyset>> {
    let filter = EventFilter {
        organization: None,
        public_only: true,
        ..filter
    };
    list_events(state, &filter, order, page).await
}

/// All events of an organization, drafts included, for its organizers. `filter`'s
/// organization and visibility are set here.
#[tracing::instrument(skip_all)]
pub async fn organization_events(
    state: &AppState,
    principal: &Principal,
    organization: OrganizationId,
    filter: EventFilter,
    order: EventOrder,
    page: PageRequest<Keyset>,
) -> ApiResult<Page<EventSummary, Keyset>> {
    state.authorize(principal, EVENTS_WRITE, Scope::Organization(organization))?;
    let filter = EventFilter {
        organization: Some(organization),
        public_only: false,
        ..filter
    };
    list_events(state, &filter, order, page).await
}

/// One page of the events matching `filter`.
async fn list_events(
    state: &AppState,
    filter: &EventFilter,
    order: EventOrder,
    page: PageRequest<Keyset>,
) -> ApiResult<Page<EventSummary, Keyset>> {
    let events = state
        .store()
        .list_events(filter, order, page.plus_one())
        .await?;
    Ok(Page::from_lookahead(events, page.limit, |event| {
        order.position(event)
    }))
}

/// Creates a draft event for an organization.
#[tracing::instrument(skip_all)]
pub async fn create_event(
    state: &AppState,
    principal: &Principal,
    organization: OrganizationId,
    new: NewEvent,
) -> ApiResult<Event> {
    state.authorize(principal, EVENTS_WRITE, Scope::Organization(organization))?;
    state
        .store()
        .organization(organization)
        .await?
        .ok_or_else(|| ApiError::not_found("organization"))?;
    let now = state.now();
    let event = Event {
        id: EventId::generate(),
        organization_id: organization,
        slug: Slug::new(new.slug)?,
        title: new.title,
        description: new.description,
        venue: new.venue,
        address: new.address,
        starts_at: new.starts_at,
        ends_at: new.ends_at,
        status: EventStatus::Draft,
        content: new.content,
        created_at: now,
        updated_at: now,
        version: 1,
    };
    event.validate()?;
    state.store().insert_event(&event).await?;
    Ok(event)
}

/// Edits an event, including publishing or cancelling it. `expected_version` is the version
/// the caller read; a different stored version fails with 412.
#[tracing::instrument(skip_all)]
pub async fn update_event(
    state: &AppState,
    principal: &Principal,
    id: EventId,
    expected_version: i64,
    changes: EventChanges,
) -> ApiResult<Event> {
    let current = writable_event(state, principal, id).await?;
    check_version(current.version, expected_version)?;
    let event = Event {
        slug: match changes.slug {
            Some(slug) => Slug::new(slug)?,
            None => current.slug,
        },
        title: changes.title.unwrap_or(current.title),
        description: changes.description.unwrap_or(current.description),
        venue: changes.venue.unwrap_or(current.venue),
        address: changes.address.unwrap_or(current.address),
        starts_at: changes.starts_at.unwrap_or(current.starts_at),
        ends_at: changes.ends_at.unwrap_or(current.ends_at),
        status: changes.status.unwrap_or(current.status),
        content: changes.content.unwrap_or(current.content),
        updated_at: state.now(),
        version: expected_version + 1,
        ..current
    };
    event.validate()?;
    state.store().update_event(&event, expected_version).await?;
    Ok(event)
}
