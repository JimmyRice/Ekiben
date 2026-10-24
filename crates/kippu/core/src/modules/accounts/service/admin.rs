//! Administration: accounts of lower roles, organizations and their members, the audit log.

use kippu_domain::account::{Account, Organization, Role};
use kippu_domain::validation::{Slug, non_empty};
use kippu_domain::{AccountId, OrganizationId};
use kippu_store::{AccountFilter, AuditRecord, Page, PageRequest};

use super::{NewAccount, insert_account};
use crate::app::AppState;
use crate::auth::{Principal, Scope};
use crate::error::{ApiError, ApiResult};
use crate::modules::accounts::permissions::{ACCOUNTS_MANAGE, AUDIT_READ, ORGANIZATIONS_MANAGE};

/// A new organization.
#[derive(Debug, Clone)]
pub struct NewOrganization {
    /// URL-friendly unique name.
    pub slug: String,
    /// Display name.
    pub name: String,
}

/// Changes to an organization. `None` keeps the stored value.
#[derive(Debug, Clone, Default)]
pub struct OrganizationChanges {
    /// URL-friendly unique name.
    pub slug: Option<String>,
    /// Display name.
    pub name: Option<String>,
}

/// An account and the organizations it belongs to.
#[derive(Debug, Clone)]
pub struct AccountDetails {
    /// The account.
    pub account: Account,
    /// The organizations it is a member of.
    pub organizations: Vec<OrganizationId>,
}

/// Creates an account of a role below the caller's own.
#[tracing::instrument(skip_all)]
pub async fn create_account(
    state: &AppState,
    principal: &Principal,
    new: NewAccount,
    role: Role,
) -> ApiResult<Account> {
    state.authorize(principal, ACCOUNTS_MANAGE, Scope::Global)?;
    if !role.is_assignable() || !principal.role().can_manage(role) {
        return Err(ApiError::forbidden());
    }
    let account = insert_account(state, new, role).await?;
    state.audit(principal, "account.create", account.id).await?;
    Ok(account)
}

/// Lists the accounts matching `filter`.
#[tracing::instrument(skip_all)]
pub async fn accounts(
    state: &AppState,
    principal: &Principal,
    filter: AccountFilter,
    page: PageRequest,
) -> ApiResult<Page<Account, uuid::Uuid>> {
    state.authorize(principal, ACCOUNTS_MANAGE, Scope::Global)?;
    let accounts = state
        .store()
        .list_accounts(&filter, page.plus_one())
        .await?;
    Ok(Page::from_lookahead(accounts, page.limit, |account| {
        account.id.as_uuid()
    }))
}

/// An account, with the organizations it belongs to.
#[tracing::instrument(skip_all)]
pub async fn account(
    state: &AppState,
    principal: &Principal,
    account_id: AccountId,
) -> ApiResult<AccountDetails> {
    state.authorize(principal, ACCOUNTS_MANAGE, Scope::Global)?;
    let account = state
        .store()
        .account(account_id)
        .await?
        .ok_or_else(|| ApiError::not_found("account"))?;
    let organizations = state.store().memberships(account_id).await?;
    Ok(AccountDetails {
        account,
        organizations,
    })
}

/// Closes an account of a role below the caller's own, as
/// [`close_own_account`](super::close_own_account) does: its records stay, nothing about the
/// person does. Closing it again changes nothing.
#[tracing::instrument(skip_all)]
pub async fn close_account(
    state: &AppState,
    principal: &Principal,
    account_id: AccountId,
) -> ApiResult<()> {
    state.authorize(principal, ACCOUNTS_MANAGE, Scope::Global)?;
    let account = state
        .store()
        .account(account_id)
        .await?
        .ok_or_else(|| ApiError::not_found("account"))?;
    if !principal.role().can_manage(account.role) {
        return Err(ApiError::forbidden());
    }
    super::profile::close(state, principal, &account).await
}

/// Creates an organization.
#[tracing::instrument(skip_all)]
pub async fn create_organization(
    state: &AppState,
    principal: &Principal,
    new: NewOrganization,
) -> ApiResult<Organization> {
    state.authorize(principal, ORGANIZATIONS_MANAGE, Scope::Global)?;
    non_empty("name", &new.name, 200)?;
    let organization = Organization {
        id: OrganizationId::generate(),
        slug: Slug::new(new.slug)?,
        name: new.name,
        created_at: state.now(),
        version: 1,
    };
    state.store().insert_organization(&organization).await?;
    state
        .audit(principal, "organization.create", organization.id)
        .await?;
    Ok(organization)
}

/// An organization, with the version to edit it by.
#[tracing::instrument(skip_all)]
pub async fn organization(
    state: &AppState,
    principal: &Principal,
    organization_id: OrganizationId,
) -> ApiResult<Organization> {
    state.authorize(principal, ORGANIZATIONS_MANAGE, Scope::Global)?;
    state
        .store()
        .organization(organization_id)
        .await?
        .ok_or_else(|| ApiError::not_found("organization"))
}

/// Renames an organization or changes its slug, if it is still at `expected_version`.
#[tracing::instrument(skip_all)]
pub async fn update_organization(
    state: &AppState,
    principal: &Principal,
    organization_id: OrganizationId,
    expected_version: i64,
    changes: OrganizationChanges,
) -> ApiResult<Organization> {
    let current = organization(state, principal, organization_id).await?;
    if current.version != expected_version {
        return Err(ApiError::stale_version());
    }
    let organization = Organization {
        slug: match changes.slug {
            Some(slug) => Slug::new(slug)?,
            None => current.slug,
        },
        name: changes.name.unwrap_or(current.name),
        version: expected_version + 1,
        ..current
    };
    non_empty("name", &organization.name, 200)?;
    state
        .store()
        .update_organization(&organization, expected_version)
        .await?;
    state
        .audit(principal, "organization.update", organization.id)
        .await?;
    Ok(organization)
}

/// Lists organizations.
#[tracing::instrument(skip_all)]
pub async fn organizations(
    state: &AppState,
    principal: &Principal,
    page: PageRequest,
) -> ApiResult<Page<Organization, uuid::Uuid>> {
    state.authorize(principal, ORGANIZATIONS_MANAGE, Scope::Global)?;
    let organizations = state.store().list_organizations(page.plus_one()).await?;
    Ok(Page::from_lookahead(
        organizations,
        page.limit,
        |organization| organization.id.as_uuid(),
    ))
}

/// The members of an organization, in account id order.
#[tracing::instrument(skip_all)]
pub async fn members(
    state: &AppState,
    principal: &Principal,
    organization_id: OrganizationId,
    page: PageRequest,
) -> ApiResult<Page<Account, uuid::Uuid>> {
    organization(state, principal, organization_id).await?;
    let members = state
        .store()
        .members(organization_id, page.plus_one())
        .await?;
    Ok(Page::from_lookahead(members, page.limit, |account| {
        account.id.as_uuid()
    }))
}

/// Makes an account a member of an organization.
#[tracing::instrument(skip_all)]
pub async fn add_member(
    state: &AppState,
    principal: &Principal,
    organization_id: OrganizationId,
    account_id: AccountId,
) -> ApiResult<()> {
    state.authorize(principal, ORGANIZATIONS_MANAGE, Scope::Global)?;
    state
        .store()
        .organization(organization_id)
        .await?
        .ok_or_else(|| ApiError::not_found("organization"))?;
    state
        .store()
        .account(account_id)
        .await?
        .ok_or_else(|| ApiError::not_found("account"))?;
    state
        .store()
        .add_member(organization_id, account_id)
        .await?;
    state
        .audit(
            principal,
            "organization.member.add",
            format!("{organization_id}/{account_id}"),
        )
        .await
}

/// Removes an account from an organization.
#[tracing::instrument(skip_all)]
pub async fn remove_member(
    state: &AppState,
    principal: &Principal,
    organization_id: OrganizationId,
    account_id: AccountId,
) -> ApiResult<()> {
    state.authorize(principal, ORGANIZATIONS_MANAGE, Scope::Global)?;
    state
        .store()
        .remove_member(organization_id, account_id)
        .await?;
    state
        .audit(
            principal,
            "organization.member.remove",
            format!("{organization_id}/{account_id}"),
        )
        .await
}

/// The audit log, newest first.
#[tracing::instrument(skip_all)]
pub async fn audit_log(
    state: &AppState,
    principal: &Principal,
    page: PageRequest<i64>,
) -> ApiResult<Page<AuditRecord, i64>> {
    state.authorize(principal, AUDIT_READ, Scope::Global)?;
    let records = state.store().audit_log(page.plus_one()).await?;
    Ok(Page::from_lookahead(records, page.limit, |record| {
        record.sequence
    }))
}
