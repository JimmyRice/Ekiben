//! HTTP handlers. Each one authenticates, authorizes, validates, then calls the store or a
//! service function — nothing more.

use crate::http::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use kippu_domain::account::{Account, Identity, Organization, Role};
use kippu_domain::validation::{Email, ProviderName, Slug, Subject, non_empty};
use kippu_domain::{AccountId, OrganizationId};
use kippu_store::Unlink;

use super::dto::{
    AuditEntryResponse, CreateAccountRequest, CreateOrganizationRequest, LoginRequest, Me,
    RefreshRequest, RegisterRequest, SessionResponse, SetEmailRequest, SetPasswordRequest,
};
use super::permissions::{ACCOUNTS_MANAGE, AUDIT_READ, ORGANIZATIONS_MANAGE, PROFILE_MANAGE};
use super::service::{set_password, validate_display_name};
use crate::app::AppState;
use crate::auth::password::{DUMMY_HASH, hash_password, validate_password, verify_password};
use crate::auth::{Principal, Scope, sessions};
use crate::error::{ApiError, ApiResult, Problem};
use crate::http::PageQuery;

const TAG: &str = "accounts";

async fn insert_account(
    state: &AppState,
    email: String,
    password: String,
    display_name: String,
    role: Role,
) -> ApiResult<Account> {
    let email = Email::new(email)?;
    validate_password(&password)?;
    validate_display_name(&display_name)?;
    let account = Account {
        id: AccountId::generate(),
        email: Some(email),
        display_name,
        role,
        created_at: state.now(),
    };
    let password_hash = hash_password(password).await?;
    state
        .store()
        .insert_account(&account, Some(&password_hash))
        .await?;
    Ok(account)
}

/// Sign up as a user.
#[utoipa::path(
    post, path = "/v1/auth/register", tag = TAG,
    request_body = RegisterRequest,
    responses(
        (status = 201, body = Account),
        (status = 409, description = "Email already registered", body = Problem),
        (status = 422, body = Problem),
    )
)]
pub(crate) async fn register(
    State(state): State<AppState>,
    Json(request): Json<RegisterRequest>,
) -> ApiResult<(StatusCode, Json<Account>)> {
    let account = insert_account(
        &state,
        request.email,
        request.password,
        request.display_name,
        Role::User,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(account)))
}

/// Sign in with email and password.
#[utoipa::path(
    post, path = "/v1/auth/login", tag = TAG,
    request_body = LoginRequest,
    responses((status = 200, body = SessionResponse), (status = 401, body = Problem))
)]
pub(crate) async fn login(
    State(state): State<AppState>,
    Json(request): Json<LoginRequest>,
) -> ApiResult<Json<SessionResponse>> {
    let invalid = || ApiError::unauthenticated("wrong email or password");
    let credentials = match Email::new(request.email) {
        Ok(email) => state.store().account_credentials(&email).await?,
        Err(_) => None,
    };
    // Verify against a dummy hash when there is no account or it has no password (it signs
    // in through an external provider), so timing reveals nothing.
    let (account, hash) = match credentials {
        Some((account, Some(hash))) => (Some(account), hash),
        _ => (None, DUMMY_HASH.clone()),
    };
    let password_matches = verify_password(request.password, hash).await?;
    match account {
        Some(account) if password_matches => Ok(Json(sessions::issue(&state, account).await?)),
        _ => Err(invalid()),
    }
}

/// Exchange a refresh token for new tokens.
#[utoipa::path(
    post, path = "/v1/auth/refresh", tag = TAG,
    request_body = RefreshRequest,
    responses((status = 200, body = SessionResponse), (status = 401, body = Problem))
)]
pub(crate) async fn refresh(
    State(state): State<AppState>,
    Json(request): Json<RefreshRequest>,
) -> ApiResult<Json<SessionResponse>> {
    Ok(Json(
        sessions::refresh(&state, &request.refresh_token).await?,
    ))
}

/// Sign out: the refresh token stops working. Access tokens lapse on their own.
#[utoipa::path(
    post, path = "/v1/auth/logout", tag = TAG,
    request_body = RefreshRequest,
    responses((status = 204))
)]
pub(crate) async fn logout(
    State(state): State<AppState>,
    Json(request): Json<RefreshRequest>,
) -> ApiResult<StatusCode> {
    sessions::revoke(&state, &request.refresh_token).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Who am I?
#[utoipa::path(
    get, path = "/v1/me", tag = TAG,
    security(("bearer" = [])),
    responses((status = 200, body = Me), (status = 401, body = Problem))
)]
pub(crate) async fn me(State(state): State<AppState>, principal: Principal) -> ApiResult<Json<Me>> {
    match principal {
        Principal::Root { key_name } => Ok(Json(Me::Root { key_name })),
        Principal::Account { id, .. } => {
            let account = state
                .store()
                .account(id)
                .await?
                .ok_or_else(|| ApiError::not_found("account"))?;
            let organizations = state.store().memberships(id).await?;
            Ok(Json(Me::Account {
                account,
                organizations,
            }))
        }
    }
}

/// The caller's own account, after checking they may manage it.
fn own_account(state: &AppState, principal: &Principal) -> ApiResult<AccountId> {
    let account = principal.require_account()?;
    state.authorize(principal, PROFILE_MANAGE, Scope::Account(account))?;
    Ok(account)
}

/// Set your email address, e.g. after signing up through a provider that shared none.
#[utoipa::path(
    put, path = "/v1/me/email", tag = TAG,
    security(("bearer" = [])),
    request_body = SetEmailRequest,
    responses((status = 200, body = Account), (status = 409, description = "Email already registered", body = Problem))
)]
pub(crate) async fn set_email(
    State(state): State<AppState>,
    principal: Principal,
    Json(request): Json<SetEmailRequest>,
) -> ApiResult<Json<Account>> {
    let account = own_account(&state, &principal)?;
    let email = Email::new(request.email)?;
    if !state.store().set_email(account, &email).await? {
        return Err(ApiError::not_found("account"));
    }
    let account = state
        .store()
        .account(account)
        .await?
        .ok_or_else(|| ApiError::not_found("account"))?;
    Ok(Json(account))
}

/// Set or change your password.
#[utoipa::path(
    put, path = "/v1/me/password", tag = TAG,
    security(("bearer" = [])),
    request_body = SetPasswordRequest,
    responses((status = 204), (status = 403, description = "Current password missing or wrong", body = Problem), (status = 422, body = Problem))
)]
pub(crate) async fn change_password(
    State(state): State<AppState>,
    principal: Principal,
    Json(request): Json<SetPasswordRequest>,
) -> ApiResult<StatusCode> {
    let account = own_account(&state, &principal)?;
    set_password(
        &state,
        account,
        request.current_password,
        request.new_password,
    )
    .await?;
    state
        .audit(&principal, "account.password.set", account)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The external sign-ins (Sign in with Apple, WeChat, …) linked to your account.
#[utoipa::path(
    get, path = "/v1/me/identities", tag = TAG,
    security(("bearer" = [])),
    responses((status = 200, body = Vec<Identity>))
)]
pub(crate) async fn list_identities(
    State(state): State<AppState>,
    principal: Principal,
) -> ApiResult<Json<Vec<Identity>>> {
    let account = own_account(&state, &principal)?;
    Ok(Json(state.store().identities(account).await?))
}

/// Unlink an external sign-in. An account always keeps a way to sign in: the last one cannot
/// be unlinked until a password is set.
#[utoipa::path(
    delete, path = "/v1/me/identities/{provider}/{subject}", tag = TAG,
    security(("bearer" = [])),
    params(("provider" = String, Path), ("subject" = String, Path)),
    responses(
        (status = 204),
        (status = 404, body = Problem),
        (status = 409, description = "The account's only way to sign in", body = Problem)
    )
)]
pub(crate) async fn unlink_identity(
    State(state): State<AppState>,
    principal: Principal,
    Path((provider, subject)): Path<(String, String)>,
) -> ApiResult<StatusCode> {
    let account = own_account(&state, &principal)?;
    let not_found = || ApiError::not_found("identity");
    let provider = ProviderName::new(provider).map_err(|_| not_found())?;
    let subject = Subject::new(subject).map_err(|_| not_found())?;
    match state
        .store()
        .unlink_identity(account, &provider, &subject)
        .await?
    {
        Unlink::Unlinked => {
            state
                .audit(
                    &principal,
                    "identity.unlink",
                    format!("{account}/{provider}"),
                )
                .await?;
            Ok(StatusCode::NO_CONTENT)
        }
        Unlink::NotLinked => Err(not_found()),
        Unlink::LastSignInMethod => Err(ApiError::new(
            StatusCode::CONFLICT,
            "last-sign-in-method",
            "this is the account's only way to sign in; set a password first",
        )),
    }
}

/// Create an account of a role below your own.
#[utoipa::path(
    post, path = "/v1/admin/accounts", tag = TAG,
    security(("bearer" = [])),
    request_body = CreateAccountRequest,
    responses((status = 201, body = Account), (status = 403, body = Problem), (status = 409, body = Problem))
)]
pub(crate) async fn create_account(
    State(state): State<AppState>,
    principal: Principal,
    Json(request): Json<CreateAccountRequest>,
) -> ApiResult<(StatusCode, Json<Account>)> {
    state.authorize(&principal, ACCOUNTS_MANAGE, Scope::Global)?;
    if !request.role.is_assignable() || !principal.role().can_manage(request.role) {
        return Err(ApiError::forbidden());
    }
    let account = insert_account(
        &state,
        request.email,
        request.password,
        request.display_name,
        request.role,
    )
    .await?;
    state
        .audit(&principal, "account.create", account.id)
        .await?;
    Ok((StatusCode::CREATED, Json(account)))
}

/// List accounts.
#[utoipa::path(
    get, path = "/v1/admin/accounts", tag = TAG,
    security(("bearer" = [])),
    params(PageQuery),
    responses((status = 200, body = Vec<Account>), (status = 403, body = Problem))
)]
pub(crate) async fn list_accounts(
    State(state): State<AppState>,
    principal: Principal,
    Query(query): Query<PageQuery>,
) -> ApiResult<Json<Vec<Account>>> {
    state.authorize(&principal, ACCOUNTS_MANAGE, Scope::Global)?;
    Ok(Json(state.store().list_accounts(query.page()).await?))
}

/// Delete an account of a role below your own.
#[utoipa::path(
    delete, path = "/v1/admin/accounts/{account_id}", tag = TAG,
    security(("bearer" = [])),
    params(("account_id" = AccountId, Path)),
    responses((status = 204), (status = 403, body = Problem), (status = 404, body = Problem))
)]
pub(crate) async fn delete_account(
    State(state): State<AppState>,
    principal: Principal,
    Path(account_id): Path<AccountId>,
) -> ApiResult<StatusCode> {
    state.authorize(&principal, ACCOUNTS_MANAGE, Scope::Global)?;
    let account = state
        .store()
        .account(account_id)
        .await?
        .ok_or_else(|| ApiError::not_found("account"))?;
    if !principal.role().can_manage(account.role) {
        return Err(ApiError::forbidden());
    }
    state.store().delete_account(account_id).await?;
    state
        .audit(&principal, "account.delete", account_id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Create an organization.
#[utoipa::path(
    post, path = "/v1/admin/organizations", tag = TAG,
    security(("bearer" = [])),
    request_body = CreateOrganizationRequest,
    responses((status = 201, body = Organization), (status = 403, body = Problem), (status = 409, body = Problem))
)]
pub(crate) async fn create_organization(
    State(state): State<AppState>,
    principal: Principal,
    Json(request): Json<CreateOrganizationRequest>,
) -> ApiResult<(StatusCode, Json<Organization>)> {
    state.authorize(&principal, ORGANIZATIONS_MANAGE, Scope::Global)?;
    non_empty("name", &request.name, 200)?;
    let organization = Organization {
        id: OrganizationId::generate(),
        slug: Slug::new(request.slug)?,
        name: request.name,
        created_at: state.now(),
    };
    state.store().insert_organization(&organization).await?;
    state
        .audit(&principal, "organization.create", organization.id)
        .await?;
    Ok((StatusCode::CREATED, Json(organization)))
}

/// List organizations.
#[utoipa::path(
    get, path = "/v1/admin/organizations", tag = TAG,
    security(("bearer" = [])),
    params(PageQuery),
    responses((status = 200, body = Vec<Organization>), (status = 403, body = Problem))
)]
pub(crate) async fn list_organizations(
    State(state): State<AppState>,
    principal: Principal,
    Query(query): Query<PageQuery>,
) -> ApiResult<Json<Vec<Organization>>> {
    state.authorize(&principal, ORGANIZATIONS_MANAGE, Scope::Global)?;
    Ok(Json(state.store().list_organizations(query.page()).await?))
}

/// Make an account a member of an organization.
#[utoipa::path(
    put, path = "/v1/admin/organizations/{organization_id}/members/{account_id}", tag = TAG,
    security(("bearer" = [])),
    params(("organization_id" = OrganizationId, Path), ("account_id" = AccountId, Path)),
    responses((status = 204), (status = 403, body = Problem), (status = 404, body = Problem))
)]
pub(crate) async fn add_member(
    State(state): State<AppState>,
    principal: Principal,
    Path((organization_id, account_id)): Path<(OrganizationId, AccountId)>,
) -> ApiResult<StatusCode> {
    state.authorize(&principal, ORGANIZATIONS_MANAGE, Scope::Global)?;
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
            &principal,
            "organization.member.add",
            format!("{organization_id}/{account_id}"),
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Remove an account from an organization.
#[utoipa::path(
    delete, path = "/v1/admin/organizations/{organization_id}/members/{account_id}", tag = TAG,
    security(("bearer" = [])),
    params(("organization_id" = OrganizationId, Path), ("account_id" = AccountId, Path)),
    responses((status = 204), (status = 403, body = Problem))
)]
pub(crate) async fn remove_member(
    State(state): State<AppState>,
    principal: Principal,
    Path((organization_id, account_id)): Path<(OrganizationId, AccountId)>,
) -> ApiResult<StatusCode> {
    state.authorize(&principal, ORGANIZATIONS_MANAGE, Scope::Global)?;
    state
        .store()
        .remove_member(organization_id, account_id)
        .await?;
    state
        .audit(
            &principal,
            "organization.member.remove",
            format!("{organization_id}/{account_id}"),
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Read the most recent audit log entries.
#[utoipa::path(
    get, path = "/v1/admin/audit-log", tag = TAG,
    security(("bearer" = [])),
    params(PageQuery),
    responses((status = 200, body = Vec<AuditEntryResponse>), (status = 403, body = Problem))
)]
pub(crate) async fn audit_log(
    State(state): State<AppState>,
    principal: Principal,
    Query(query): Query<PageQuery>,
) -> ApiResult<Json<Vec<AuditEntryResponse>>> {
    state.authorize(&principal, AUDIT_READ, Scope::Global)?;
    let entries = state.store().audit_log(query.page().limit).await?;
    Ok(Json(
        entries
            .into_iter()
            .map(|entry| AuditEntryResponse {
                at: entry.at,
                actor: entry.actor,
                action: entry.action,
                target: entry.target,
            })
            .collect(),
    ))
}
