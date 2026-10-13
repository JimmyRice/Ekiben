//! Organizations as the public sees them: those with a published or cancelled event. Their
//! organizers (and admins) see their own organizations from the start.

use kippu_domain::OrganizationId;
use kippu_domain::account::Organization;
use kippu_store::{Page, PageRequest};

use super::can_write;
use crate::app::AppState;
use crate::auth::Principal;
use crate::error::{ApiError, ApiResult};

/// The organizations with a published or cancelled event, in id order.
#[tracing::instrument(skip_all)]
pub async fn public_organizations(
    state: &AppState,
    page: PageRequest,
) -> ApiResult<Page<Organization, uuid::Uuid>> {
    let organizations = state.store().public_organizations(page.plus_one()).await?;
    Ok(Page::from_lookahead(
        organizations,
        page.limit,
        |organization| organization.id.as_uuid(),
    ))
}

/// An organization the caller may see: one with a published or cancelled event, or their
/// own. Others are reported as missing.
#[tracing::instrument(skip_all)]
pub async fn visible_organization(
    state: &AppState,
    principal: Option<&Principal>,
    id: OrganizationId,
) -> ApiResult<Organization> {
    let store = state.store();
    let organization = if can_write(state, principal, id) {
        store.organization(id).await?
    } else {
        store.public_organization(id).await?
    };
    organization.ok_or_else(|| ApiError::not_found("organization"))
}
