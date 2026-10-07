//! Denial use cases, independent of HTTP: keeping deny lists, and listing what gates refuse.

use kippu_domain::catalog::Event;
use kippu_domain::denial::{Denial, DenialSubject, DeniedTicket};
use kippu_domain::{DenialId, EventId, OrganizationId, ValidationError};
use kippu_store::{Insertion, Keyset, Page, PageRequest};

use crate::app::AppState;
use crate::auth::{Permission, Principal, Scope};
use crate::error::{ApiError, ApiResult};
use crate::modules::catalog::service::visible_event;
use crate::modules::denials::permissions::{DENIALS_MANAGE, DENIALS_READ};

/// A denial to add.
#[derive(Debug, Clone)]
pub struct NewDenial {
    /// The ticket or account refused entry.
    pub subject: DenialSubject,
    /// Why, for staff.
    pub note: String,
}

/// Changes to a denial. `None` keeps the stored value.
#[derive(Debug, Clone, Default)]
pub struct DenialChanges {
    /// Why, for staff.
    pub note: Option<String>,
}

/// A denial that was asked for, and whether it is new: adding the same denial again returns
/// the one already there, unchanged.
#[derive(Debug, Clone)]
pub struct AddedDenial {
    /// The denial.
    pub denial: Denial,
    /// Whether this call added it.
    pub created: bool,
}

/// An event whose denials the caller may act on with `permission`.
async fn event_for(
    state: &AppState,
    principal: &Principal,
    permission: Permission,
    id: EventId,
) -> ApiResult<Event> {
    let event = visible_event(state, Some(principal), id).await?;
    state.authorize(
        principal,
        permission,
        Scope::Organization(event.organization_id),
    )?;
    Ok(event)
}

/// An organization whose denials the caller may act on with `permission`.
async fn organization_for(
    state: &AppState,
    principal: &Principal,
    permission: Permission,
    id: OrganizationId,
) -> ApiResult<OrganizationId> {
    state.authorize(principal, permission, Scope::Organization(id))?;
    state
        .store()
        .organization(id)
        .await?
        .ok_or_else(|| ApiError::not_found("organization"))?;
    Ok(id)
}

/// Checks that a denied ticket is one of `event`'s (or, without an event, of the
/// organization's events), and that a denied account exists.
async fn check_subject(
    state: &AppState,
    organization: OrganizationId,
    event: Option<&Event>,
    subject: DenialSubject,
) -> ApiResult<()> {
    match subject {
        DenialSubject::Ticket(id) => {
            let ours = match state.store().ticket(id).await? {
                None => false,
                Some(ticket) => match event {
                    Some(event) => ticket.event_id == event.id,
                    None => state
                        .store()
                        .event(ticket.event_id)
                        .await?
                        .is_some_and(|event| event.organization_id == organization),
                },
            };
            if !ours {
                return Err(ValidationError::new(
                    "subject.id",
                    if event.is_some() {
                        "is not a ticket of this event"
                    } else {
                        "is not a ticket of this organization's events"
                    },
                )
                .into());
            }
        }
        DenialSubject::Account(id) => {
            if state.store().account(id).await?.is_none() {
                return Err(ValidationError::new("subject.id", "is not an account").into());
            }
        }
    }
    Ok(())
}

/// Adds a denial for `event`, or with `event` `None` for every event of `organization`.
async fn add(
    state: &AppState,
    principal: &Principal,
    organization: OrganizationId,
    event: Option<&Event>,
    new: NewDenial,
) -> ApiResult<AddedDenial> {
    check_subject(state, organization, event, new.subject).await?;
    let now = state.now();
    let event_id = event.map(|event| event.id);
    let denial = Denial {
        id: DenialId::derive(organization, event_id, new.subject),
        organization_id: organization,
        event_id,
        subject: new.subject,
        note: new.note,
        created_at: now,
        updated_at: now,
        version: 1,
    };
    denial.validate()?;
    match state.store().insert_denial(&denial).await? {
        Insertion::Inserted => {
            state.audit(principal, "denial.create", denial.id).await?;
            tracing::info!(
                denial = %denial.id,
                subject = denial.subject.kind(),
                event = ?denial.event_id,
                "denial added"
            );
            Ok(AddedDenial {
                denial,
                created: true,
            })
        }
        Insertion::Existing(existing) => Ok(AddedDenial {
            denial: existing,
            created: false,
        }),
    }
}

/// Denies a ticket or an account entry to every event of an organization, present and
/// future. Adding the same denial again changes nothing.
#[tracing::instrument(skip_all)]
pub async fn create_organization_denial(
    state: &AppState,
    principal: &Principal,
    organization: OrganizationId,
    new: NewDenial,
) -> ApiResult<AddedDenial> {
    let organization = organization_for(state, principal, DENIALS_MANAGE, organization).await?;
    add(state, principal, organization, None, new).await
}

/// Denies a ticket or an account entry to one event. Adding the same denial again changes
/// nothing.
#[tracing::instrument(skip_all)]
pub async fn create_event_denial(
    state: &AppState,
    principal: &Principal,
    event: EventId,
    new: NewDenial,
) -> ApiResult<AddedDenial> {
    let event = event_for(state, principal, DENIALS_MANAGE, event).await?;
    add(state, principal, event.organization_id, Some(&event), new).await
}

/// A denial of an organization the caller may act on with `permission`. Others' denials look
/// missing.
async fn denial_for(
    state: &AppState,
    principal: &Principal,
    permission: Permission,
    id: DenialId,
) -> ApiResult<Denial> {
    let denial = state
        .store()
        .denial(id)
        .await?
        .ok_or_else(|| ApiError::not_found("denial"))?;
    if state.policy().permits(
        principal,
        permission,
        Scope::Organization(denial.organization_id),
    ) {
        Ok(denial)
    } else {
        Err(ApiError::not_found("denial"))
    }
}

/// A denial.
#[tracing::instrument(skip_all)]
pub async fn denial(state: &AppState, principal: &Principal, id: DenialId) -> ApiResult<Denial> {
    denial_for(state, principal, DENIALS_READ, id).await
}

/// Changes a denial's note. `expected_version` is the version the caller read.
#[tracing::instrument(skip_all)]
pub async fn update_denial(
    state: &AppState,
    principal: &Principal,
    id: DenialId,
    expected_version: i64,
    changes: DenialChanges,
) -> ApiResult<Denial> {
    let current = denial_for(state, principal, DENIALS_MANAGE, id).await?;
    if current.version != expected_version {
        return Err(ApiError::stale_version());
    }
    let updated = Denial {
        note: changes.note.unwrap_or_else(|| current.note.clone()),
        updated_at: state.now(),
        version: expected_version + 1,
        ..current
    };
    updated.validate()?;
    state
        .store()
        .update_denial(&updated, expected_version)
        .await?;
    state.audit(principal, "denial.update", updated.id).await?;
    Ok(updated)
}

/// Lifts a denial: its ticket, or its account's tickets, are admitted again (unless revoked
/// or denied otherwise).
#[tracing::instrument(skip_all)]
pub async fn delete_denial(state: &AppState, principal: &Principal, id: DenialId) -> ApiResult<()> {
    let denial = denial_for(state, principal, DENIALS_MANAGE, id).await?;
    if state.store().delete_denial(denial.id).await? {
        state.audit(principal, "denial.delete", denial.id).await?;
        tracing::info!(denial = %denial.id, "denial lifted");
    }
    Ok(())
}

/// An organization's denials for all of its events, most recently added first.
#[tracing::instrument(skip_all)]
pub async fn organization_denials(
    state: &AppState,
    principal: &Principal,
    organization: OrganizationId,
    page: PageRequest<Keyset>,
) -> ApiResult<Page<Denial, Keyset>> {
    let organization = organization_for(state, principal, DENIALS_READ, organization).await?;
    let denials = state
        .store()
        .organization_denials(organization, page.plus_one())
        .await?;
    Ok(Page::from_lookahead(denials, page.limit, position))
}

/// An event's own denials (not its organization's), most recently added first.
#[tracing::instrument(skip_all)]
pub async fn event_denials(
    state: &AppState,
    principal: &Principal,
    event: EventId,
    page: PageRequest<Keyset>,
) -> ApiResult<Page<Denial, Keyset>> {
    let event = event_for(state, principal, DENIALS_READ, event).await?;
    let denials = state
        .store()
        .event_denials(event.id, page.plus_one())
        .await?;
    Ok(Page::from_lookahead(denials, page.limit, position))
}

fn position(denial: &Denial) -> Keyset {
    Keyset {
        at: denial.created_at,
        id: denial.id.as_uuid(),
    }
}

/// The tickets of an event a gate must refuse, in ticket id order: revoked tickets, denied
/// tickets and the tickets of denied accounts, by the event's list or its organization's.
#[tracing::instrument(skip_all)]
pub async fn denied_tickets(
    state: &AppState,
    principal: &Principal,
    event: EventId,
    page: PageRequest<uuid::Uuid>,
) -> ApiResult<Page<DeniedTicket, uuid::Uuid>> {
    let event = event_for(state, principal, DENIALS_READ, event).await?;
    let tickets = state
        .store()
        .denied_tickets(event.id, page.plus_one())
        .await?;
    Ok(Page::from_lookahead(tickets, page.limit, |denied| {
        denied.ticket_id.as_uuid()
    }))
}
