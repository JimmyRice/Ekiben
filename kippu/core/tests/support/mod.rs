//! An in-process Kippu instance on a temporary SQLite database, driven through its router.
#![allow(
    dead_code,
    unreachable_pub,
    clippy::unwrap_used,
    clippy::missing_panics_doc,
    reason = "shared test support; not every test uses every helper"
)]

use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use ed25519_dalek::SigningKey;
use kippu_core::auth::tokens::mint_root_token;
use kippu_core::config::{
    AuthConfig, Config, DatabaseConfig, IssuerConfig, KeysConfig, RootConfig, RootKey,
    ServerConfig, WorkersConfig,
};
use kippu_core::{App, Kippu, ManualClock};
use kippu_domain::{Duration, Timestamp};
use kippu_store::Store;
use kippu_store_sqlite::SqliteStore;
use serde_json::{Value, json};
use tower::ServiceExt;

pub const ISSUER: &str = "kippu.test";

pub struct TestApp {
    pub app: App,
    pub clock: Arc<ManualClock>,
    root_key: SigningKey,
    _dir: tempfile::TempDir,
}

pub struct Reply {
    pub status: StatusCode,
    pub headers: axum::http::HeaderMap,
    pub body: Value,
}

impl TestApp {
    pub async fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let url = format!("sqlite://{}", dir.path().join("kippu.db").display());
        let store = SqliteStore::connect(&url).await.unwrap();
        store.migrate().await.unwrap();

        let root_key = SigningKey::from_bytes(&[42; 32]);
        let config = Config {
            server: ServerConfig::default(),
            database: DatabaseConfig { url: url.into() },
            issuer: IssuerConfig {
                id: ISSUER.to_owned(),
            },
            keys: KeysConfig {
                ticket_signing_key: STANDARD.encode([1; 32]).into(),
                retired_ticket_keys: Vec::new(),
                token_signing_key: STANDARD.encode([2; 32]).into(),
            },
            root: RootConfig {
                keys: vec![RootKey {
                    name: "owner".to_owned(),
                    public_key: STANDARD.encode(root_key.verifying_key().as_bytes()),
                }],
                max_token_ttl_seconds: 600,
            },
            auth: AuthConfig::default(),
            workers: WorkersConfig::default(),
        };
        let clock = Arc::new(ManualClock::new(Timestamp::from_unix_seconds(
            1_798_761_600,
        ))); // 2027-01-01
        let app = Kippu::new()
            .modules(kippu_core::default_modules())
            .build(config, Arc::new(store), clock.clone())
            .unwrap();
        Self {
            app,
            clock,
            root_key,
            _dir: dir,
        }
    }

    pub fn now(&self) -> Timestamp {
        self.app.state().now()
    }

    pub fn root_token(&self) -> String {
        mint_root_token(
            &self.root_key,
            "owner",
            ISSUER,
            self.now(),
            Duration::minutes(5),
        )
    }

    pub async fn call(
        &self,
        method: Method,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> Reply {
        self.call_with(method, path, token, body, &[]).await
    }

    pub async fn call_with(
        &self,
        method: Method,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
        headers: &[(&str, &str)],
    ) -> Reply {
        let mut request = Request::builder().method(method).uri(path);
        if let Some(token) = token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let request = match body {
            Some(body) => request
                .header("content-type", "application/json")
                .body(Body::from(body.to_string())),
            None => request.body(Body::empty()),
        }
        .unwrap();
        let response = self.app.router().oneshot(request).await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        Reply {
            status,
            headers,
            body,
        }
    }

    /// Creates an account through the admin API as root and signs it in.
    pub async fn account(&self, role: &str, email: &str) -> (String, Value) {
        let created = self
            .call(
                Method::POST,
                "/v1/admin/accounts",
                Some(&self.root_token()),
                Some(json!({ "email": email, "password": "correct horse battery", "display_name": role, "role": role })),
            )
            .await;
        assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);
        self.login(email).await
    }

    /// Signs in and returns the access token and the account.
    pub async fn login(&self, email: &str) -> (String, Value) {
        let session = self
            .call(
                Method::POST,
                "/v1/auth/login",
                None,
                Some(json!({ "email": email, "password": "correct horse battery" })),
            )
            .await;
        assert_eq!(session.status, StatusCode::OK, "{:?}", session.body);
        let token = session.body["access_token"]["token"]
            .as_str()
            .unwrap()
            .to_owned();
        (token, session.body["account"].clone())
    }
}
