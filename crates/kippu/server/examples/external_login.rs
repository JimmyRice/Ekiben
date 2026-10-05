//! A `kippu` binary with one extra way to sign in: a pretend OAuth provider.
//!
//! It shows the whole shape of an external sign-in module — the redirect to the provider, the
//! `state` check against CSRF, PKCE, the code exchange, and finally `link_or_create` plus
//! `sessions::issue` — with a provider that approves everyone, so it runs without accounts at
//! Apple or WeChat. See `docs/external-login.md`.
//!
//! ```text
//! cargo run -p kippu-server --example external_login -- serve --config kippu.toml
//! open http://localhost:8080/v1/auth/pretend/start?name=Miku
//! ```
//!
//! The browser goes to the provider, comes back to `/v1/auth/pretend/callback` and receives a
//! Kippu session as JSON. Sign in again with the same name and you get the same account.
#![allow(clippy::print_stdout, reason = "an example")]

use std::process::ExitCode;

use axum::extract::{Query, State};
use axum::http::header::{COOKIE, LOCATION, SET_COOKIE};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use kippu_core::auth::external::{self, ExternalIdentity};
use kippu_core::auth::sessions;
use kippu_core::http::Json;
use kippu_core::{ApiError, ApiResult, AppState, Module};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use utoipa_axum::router::OpenApiRouter;

/// The cookie that carries `state` and the PKCE verifier from `start` to `callback`. It lives
/// in the browser, so the server stays stateless; tampering with it only breaks the
/// tamperer's own sign-in.
const COOKIE_NAME: &str = "kippu_pretend_login";

/// The module: `/v1/auth/pretend/start` and `/v1/auth/pretend/callback`, plus the pretend
/// provider's own page under `/pretend-provider`.
struct PretendLogin;

impl Module for PretendLogin {
    fn name(&self) -> &'static str {
        "pretend-login"
    }

    fn routes(&self) -> OpenApiRouter<AppState> {
        OpenApiRouter::new()
            .route("/v1/auth/pretend/start", get(start))
            .route("/v1/auth/pretend/callback", get(callback))
            .route("/pretend-provider/authorize", get(provider_authorize))
    }
}

fn random() -> ApiResult<String> {
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes).map_err(|error| ApiError::internal(error.to_string()))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn redirect(location: &str) -> ApiResult<Response> {
    let location = HeaderValue::from_str(location).map_err(ApiError::internal)?;
    Ok((StatusCode::FOUND, [(LOCATION, location)]).into_response())
}

#[derive(Deserialize)]
struct StartQuery {
    /// Passed through to the pretend provider, which signs in whoever asks.
    name: Option<String>,
}

/// Step 1: remember `state` and a PKCE verifier in a cookie, send the browser to the provider.
async fn start(Query(query): Query<StartQuery>) -> ApiResult<Response> {
    let state = random()?;
    let verifier = random()?;
    let name = query.name.unwrap_or_else(|| "guest".to_owned());
    let mut response = redirect(&format!(
        "/pretend-provider/authorize?state={state}&code_challenge={}&login_hint={}",
        challenge(&verifier),
        URL_SAFE_NO_PAD.encode(name)
    ))?;
    let cookie = format!(
        "{COOKIE_NAME}={state}.{verifier}; Path=/v1/auth/pretend; Max-Age=600; HttpOnly; SameSite=Lax"
    );
    response.headers_mut().insert(
        SET_COOKIE,
        HeaderValue::from_str(&cookie).map_err(ApiError::internal)?,
    );
    Ok(response)
}

#[derive(Deserialize)]
struct AuthorizeQuery {
    state: String,
    code_challenge: String,
    login_hint: String,
}

/// The pretend provider: approves at once and returns a code bound to the PKCE challenge.
/// A real provider shows its own sign-in page here and keeps the code on its side.
async fn provider_authorize(Query(query): Query<AuthorizeQuery>) -> ApiResult<Response> {
    let code = URL_SAFE_NO_PAD.encode(format!("{}|{}", query.login_hint, query.code_challenge));
    redirect(&format!(
        "/v1/auth/pretend/callback?code={code}&state={}",
        query.state
    ))
}

#[derive(Deserialize)]
struct CallbackQuery {
    code: String,
    state: String,
}

/// Step 2: check `state`, exchange the code with the PKCE verifier, sign the person in.
async fn callback(
    State(app): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<CallbackQuery>,
) -> ApiResult<Json<sessions::SessionResponse>> {
    let rejected = || ApiError::unauthenticated("sign-in was not started here, or expired");
    let (state, verifier) = headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .find_map(|pair| pair.trim().strip_prefix(&format!("{COOKIE_NAME}=")))
        .and_then(|value| value.split_once('.'))
        .ok_or_else(rejected)?;
    if state != query.state {
        return Err(rejected());
    }

    // The "token exchange". A real module POSTs `code` and `verifier` to the provider over
    // HTTPS and checks the signature of what comes back (e.g. Apple's id_token).
    let exchanged = URL_SAFE_NO_PAD
        .decode(&query.code)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .ok_or_else(rejected)?;
    let (hint, bound_challenge) = exchanged.split_once('|').ok_or_else(rejected)?;
    if bound_challenge != challenge(verifier) {
        return Err(rejected());
    }
    let name = URL_SAFE_NO_PAD
        .decode(hint)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .ok_or_else(rejected)?;

    // What the provider vouches for. The subject must be the provider's stable id for the
    // person; this pretend provider simply uses the name.
    let identity =
        ExternalIdentity::new("pretend", format!("user:{name}")).display_name(name.clone());
    let linked = external::link_or_create(&app, identity).await?;
    if linked.created {
        tracing::info!(account = %linked.account.id, "account created by an external sign-in");
    }
    Ok(Json(sessions::issue(&app, linked.account).await?))
}

fn main() -> ExitCode {
    kippu_server::Launcher::new()
        .modules(kippu_core::default_modules())
        .module(PretendLogin)
        .run()
}
