//! HTTP handlers: each turns a request into one [`service`](super::service) call (or a
//! [`sessions`] call) and its result into a response. Rules live in the service, not here.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use kippu_domain::account::{Account, Identity, Organization};
use kippu_domain::{AccountId, OrganizationId};

use super::dto::{
    AuditEntryResponse, CreateAccountRequest, CreateOrganizationRequest, LoginRequest, Me,
    RefreshRequest, RegisterRequest, SessionResponse, SetEmailRequest, SetPasswordRequest,
};
use super::service;
use crate::app::AppState;
use crate::auth::{Principal, sessions};
use crate::error::{ApiResult, Problem};
use crate::http::{Json, Listing, ListingTag, PageQuery};

const TAG: &str = "accounts";

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
    let account = service::register(&state, request.into()).await?;
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
    Ok(Json(
        service::login(&state, request.email, request.password).await?,
    ))
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
    Ok(Json(service::whoami(&state, &principal).await?.into()))
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
    Ok(Json(
        service::set_email(&state, &principal, request.email).await?,
    ))
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
    service::change_password(
        &state,
        &principal,
        request.current_password,
        request.new_password,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The external sign-ins (Sign in with Apple, WeChat, …) linked to your account.
#[utoipa::path(
    get, path = "/v1/me/identities", tag = TAG,
    security(("bearer" = [])),
    responses((status = 200, body = Listing<Identity>))
)]
pub(crate) async fn list_identities(
    State(state): State<AppState>,
    principal: Principal,
) -> ApiResult<Json<Listing<Identity>>> {
    Ok(Json(Listing::all(
        service::identities(&state, &principal).await?,
    )))
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
    service::unlink_identity(&state, &principal, provider, subject).await?;
    Ok(StatusCode::NO_CONTENT)
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
    let (new, role) = request.into_parts();
    let account = service::create_account(&state, &principal, new, role).await?;
    Ok((StatusCode::CREATED, Json(account)))
}

/// List accounts.
#[utoipa::path(
    get, path = "/v1/admin/accounts", tag = TAG,
    security(("bearer" = [])),
    params(PageQuery),
    responses(
        (status = 200, body = Listing<Account>),
        (status = 400, body = Problem),
        (status = 403, body = Problem)
    )
)]
pub(crate) async fn list_accounts(
    State(state): State<AppState>,
    principal: Principal,
    Query(query): Query<PageQuery>,
) -> ApiResult<Json<Listing<Account>>> {
    let page = query.page(ListingTag::ACCOUNTS)?;
    let accounts = service::accounts(&state, &principal, page).await?;
    Ok(Json(Listing::page(
        accounts,
        ListingTag::ACCOUNTS,
        |account| account,
    )))
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
    service::delete_account(&state, &principal, account_id).await?;
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
    let organization = service::create_organization(&state, &principal, request.into()).await?;
    Ok((StatusCode::CREATED, Json(organization)))
}

/// List organizations.
#[utoipa::path(
    get, path = "/v1/admin/organizations", tag = TAG,
    security(("bearer" = [])),
    params(PageQuery),
    responses(
        (status = 200, body = Listing<Organization>),
        (status = 400, body = Problem),
        (status = 403, body = Problem)
    )
)]
pub(crate) async fn list_organizations(
    State(state): State<AppState>,
    principal: Principal,
    Query(query): Query<PageQuery>,
) -> ApiResult<Json<Listing<Organization>>> {
    let page = query.page(ListingTag::ORGANIZATIONS)?;
    let organizations = service::organizations(&state, &principal, page).await?;
    Ok(Json(Listing::page(
        organizations,
        ListingTag::ORGANIZATIONS,
        |organization| organization,
    )))
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
    service::add_member(&state, &principal, organization_id, account_id).await?;
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
    service::remove_member(&state, &principal, organization_id, account_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Read the audit log, newest first.
#[utoipa::path(
    get, path = "/v1/admin/audit-log", tag = TAG,
    security(("bearer" = [])),
    params(PageQuery),
    responses(
        (status = 200, body = Listing<AuditEntryResponse>),
        (status = 400, body = Problem),
        (status = 403, body = Problem)
    )
)]
pub(crate) async fn audit_log(
    State(state): State<AppState>,
    principal: Principal,
    Query(query): Query<PageQuery>,
) -> ApiResult<Json<Listing<AuditEntryResponse>>> {
    let page = query.page(ListingTag::AUDIT_LOG)?;
    let records = service::audit_log(&state, &principal, page).await?;
    Ok(Json(Listing::page(
        records,
        ListingTag::AUDIT_LOG,
        AuditEntryResponse::from,
    )))
}
