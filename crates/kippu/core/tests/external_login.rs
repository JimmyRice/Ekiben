//! External sign-in: a module verifies someone at a provider, then `link_or_create` and
//! `sessions::issue` turn them into a Kippu session.
#![allow(
    clippy::unwrap_used,
    unreachable_pub,
    reason = "test module and helpers: a failure is the test failing"
)]

mod support;

use std::sync::Arc;

use axum::extract::State;
use axum::http::{Method, StatusCode};
use kippu_core::auth::external::{self, ExternalIdentity};
use kippu_core::auth::sessions::{self, SessionResponse};
use kippu_core::http::Json;
use kippu_core::{ApiResult, AppState, Module, Principal};
use serde::Deserialize;
use serde_json::{Value, json};
use support::{TestApp, uuid_suffix};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

/// What the pretend provider vouches for. A real module learns this from the provider's
/// signed response, never from the client.
#[derive(Deserialize, utoipa::ToSchema)]
struct Vouched {
    subject: String,
    email: Option<String>,
    #[serde(default)]
    email_verified: bool,
    name: Option<String>,
}

/// Sign in through the pretend provider.
#[utoipa::path(post, path = "/v1/auth/pretend", request_body = Vouched, responses((status = 200)))]
async fn pretend_sign_in(
    State(state): State<AppState>,
    Json(vouched): Json<Vouched>,
) -> ApiResult<Json<SessionResponse>> {
    let mut identity = ExternalIdentity::new("pretend", vouched.subject);
    if let Some(email) = vouched.email {
        identity = identity.email(email, vouched.email_verified);
    }
    if let Some(name) = vouched.name {
        identity = identity.display_name(name);
    }
    let linked = external::link_or_create(&state, identity).await?;
    Ok(Json(sessions::issue(&state, linked.account).await?))
}

/// Link the pretend provider to the signed-in account.
#[utoipa::path(post, path = "/v1/auth/pretend/link", request_body = Vouched, responses((status = 204)))]
async fn pretend_link(
    State(state): State<AppState>,
    principal: Principal,
    Json(vouched): Json<Vouched>,
) -> ApiResult<StatusCode> {
    let account = principal.require_account()?;
    let account = state.store().account(account).await?.unwrap();
    external::link(
        &state,
        &account,
        &ExternalIdentity::new("pretend", vouched.subject),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

struct Pretend;

impl Module for Pretend {
    fn name(&self) -> &'static str {
        "pretend-login"
    }

    fn routes(&self) -> OpenApiRouter<AppState> {
        OpenApiRouter::new()
            .routes(routes!(pretend_sign_in))
            .routes(routes!(pretend_link))
    }
}

async fn app(link_by_verified_email: bool) -> TestApp {
    TestApp::start_with(vec![Arc::new(Pretend)], |config| {
        config.auth.link_by_verified_email = link_by_verified_email;
    })
    .await
}

async fn sign_in(app: &TestApp, vouched: Value) -> support::Reply {
    app.call(Method::POST, "/v1/auth/pretend", None, Some(vouched))
        .await
}

fn token(reply: &support::Reply) -> String {
    reply.body["access_token"]["token"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn a_first_sign_in_creates_an_account_without_email_or_password() {
    let app = app(false).await;
    let subject = format!("wx-{}", uuid_suffix());
    let first = sign_in(&app, json!({ "subject": subject, "name": "  Miku  " })).await;
    assert_eq!(first.status, StatusCode::OK, "{:?}", first.body);
    let account = &first.body["account"];
    assert_eq!(account["email"], Value::Null);
    assert_eq!(account["display_name"], "Miku");
    assert_eq!(account["role"], "user");

    let again = sign_in(&app, json!({ "subject": subject })).await;
    assert_eq!(
        again.body["account"]["id"], account["id"],
        "same person, same account"
    );

    let me = app
        .call(Method::GET, "/v1/me", Some(&token(&again)), None)
        .await;
    assert_eq!(me.body["account"]["id"], account["id"]);
    let identities = app
        .call(Method::GET, "/v1/me/identities", Some(&token(&again)), None)
        .await;
    assert_eq!(identities.body["items"][0]["provider"], "pretend");
    assert_eq!(identities.body["items"][0]["subject"], subject);

    let audit = app
        .call(
            Method::GET,
            "/v1/admin/audit-log",
            Some(&app.root_token()),
            None,
        )
        .await;
    assert!(
        audit.body["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["action"] == "identity.link")
    );
}

#[tokio::test]
async fn unverified_emails_are_not_stored_and_taken_ones_are_refused() {
    let app = app(false).await;
    let unverified = sign_in(
        &app,
        json!({ "subject": uuid_suffix(), "email": "someone@example.org" }),
    )
    .await;
    assert_eq!(unverified.body["account"]["email"], Value::Null);

    let buyer_email = format!("buyer-{}@example.org", uuid_suffix());
    app.call(
        Method::POST,
        "/v1/auth/register",
        None,
        Some(json!({ "email": buyer_email, "password": "correct horse battery", "display_name": "Buyer" })),
    )
    .await;
    let clash = sign_in(
        &app,
        json!({ "subject": uuid_suffix(), "email": buyer_email, "email_verified": true }),
    )
    .await;
    assert_eq!(clash.status, StatusCode::CONFLICT);
    assert_eq!(
        clash.body["type"],
        "urn:kippu:problem:email-already-registered"
    );
}

#[tokio::test]
async fn verified_emails_link_to_existing_accounts_only_when_enabled() {
    let app = app(true).await;
    let email = format!("fan-{}@example.org", uuid_suffix());
    app.call(
        Method::POST,
        "/v1/auth/register",
        None,
        Some(json!({ "email": email, "password": "correct horse battery", "display_name": "Fan" })),
    )
    .await;
    let (_, registered) = app.login(&email).await;

    let unverified = sign_in(
        &app,
        json!({ "subject": uuid_suffix(), "email": email, "email_verified": false }),
    )
    .await;
    assert_ne!(unverified.body["account"]["id"], registered["id"]);

    let verified = sign_in(
        &app,
        json!({ "subject": uuid_suffix(), "email": email, "email_verified": true }),
    )
    .await;
    assert_eq!(verified.status, StatusCode::OK, "{:?}", verified.body);
    assert_eq!(verified.body["account"]["id"], registered["id"]);
}

#[tokio::test]
async fn signed_in_accounts_link_providers_and_keep_a_way_to_sign_in() {
    let app = app(false).await;
    let subject = uuid_suffix();
    let social = sign_in(&app, json!({ "subject": subject })).await;
    let social_token = token(&social);
    let unlink_path = format!("/v1/me/identities/pretend/{subject}");

    let last = app
        .call(Method::DELETE, &unlink_path, Some(&social_token), None)
        .await;
    assert_eq!(last.status, StatusCode::CONFLICT);
    assert_eq!(last.body["type"], "urn:kippu:problem:last-sign-in-method");

    // A password first (none to confirm yet), then the provider can go.
    let password = app
        .call(
            Method::PUT,
            "/v1/me/password",
            Some(&social_token),
            Some(json!({ "new_password": "a brand new password" })),
        )
        .await;
    assert_eq!(
        password.status,
        StatusCode::NO_CONTENT,
        "{:?}",
        password.body
    );
    let changing = app
        .call(
            Method::PUT,
            "/v1/me/password",
            Some(&social_token),
            Some(json!({ "new_password": "another new password" })),
        )
        .await;
    assert_eq!(changing.body["type"], "urn:kippu:problem:wrong-password");
    let email = format!("social-{}@example.org", uuid_suffix());
    let with_email = app
        .call(
            Method::PUT,
            "/v1/me/email",
            Some(&social_token),
            Some(json!({ "email": email })),
        )
        .await;
    assert_eq!(with_email.body["email"], email);
    let session = app
        .call(
            Method::POST,
            "/v1/auth/login",
            None,
            Some(json!({ "email": email, "password": "a brand new password" })),
        )
        .await;
    assert_eq!(session.status, StatusCode::OK);
    assert_eq!(
        app.call(Method::DELETE, &unlink_path, Some(&social_token), None)
            .await
            .status,
        StatusCode::NO_CONTENT
    );

    // Linking: idempotent for the owner, refused for anyone else.
    let buyer = app.buyer().await;
    let other = uuid_suffix();
    let link = |token: String, subject: String| {
        let app = &app;
        async move {
            app.call(
                Method::POST,
                "/v1/auth/pretend/link",
                Some(&token),
                Some(json!({ "subject": subject })),
            )
            .await
        }
    };
    assert_eq!(
        link(buyer.clone(), other.clone()).await.status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        link(buyer.clone(), other.clone()).await.status,
        StatusCode::NO_CONTENT
    );
    let taken = link(social_token, other).await;
    assert_eq!(
        taken.body["type"],
        "urn:kippu:problem:identity-linked-elsewhere"
    );
}
