//! Administration: accounts of lower roles, organizations and their members, the audit log.

use kippu_domain::account::{Account, Organization, Role};
use kippu_domain::validation::{Slug, non_empty};
use kippu_domain::{AccountId, OrganizationId};
use kippu_store::{AuditEntry, PageRequest};

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

/// Creates an account of a role below the caller's own.
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

/// Lists accounts.
pub async fn accounts(
    state: &AppState,
    principal: &Principal,
    page: PageRequest,
) -> ApiResult<Vec<Account>> {
    state.authorize(principal, ACCOUNTS_MANAGE, Scope::Global)?;
    Ok(state.store().list_accounts(page).await?)
}

/// Deletes an account of a role below the caller's own.
pub async fn delete_account(
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
    state.store().delete_account(account_id).await?;
    state.audit(principal, "account.delete", account_id).await
}

/// Creates an organization.
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
    };
    state.store().insert_organization(&organization).await?;
    state
        .audit(principal, "organization.create", organization.id)
        .await?;
    Ok(organization)
}

/// Lists organizations.
pub async fn organizations(
    state: &AppState,
    principal: &Principal,
    page: PageRequest,
) -> ApiResult<Vec<Organization>> {
    state.authorize(principal, ORGANIZATIONS_MANAGE, Scope::Global)?;
    Ok(state.store().list_organizations(page).await?)
}

/// Makes an account a member of an organization.
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

/// The most recent audit log entries, newest first.
pub async fn audit_log(
    state: &AppState,
    principal: &Principal,
    limit: u32,
) -> ApiResult<Vec<AuditEntry>> {
    state.authorize(principal, AUDIT_READ, Scope::Global)?;
    Ok(state.store().audit_log(limit).await?)
}
