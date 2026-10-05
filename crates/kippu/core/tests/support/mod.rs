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
            database: DatabaseConfig {
                url: url.into(),
                auto_migrate: true,
            },
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
        self.send(request).await
    }

    /// Sends `body` as is, with exactly the given headers.
    pub async fn call_raw(
        &self,
        method: Method,
        path: &str,
        headers: &[(&str, &str)],
        body: impl Into<Body>,
    ) -> Reply {
        let mut request = Request::builder().method(method).uri(path);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        self.send(request.body(body.into()).unwrap()).await
    }

    async fn send(&self, request: Request<Body>) -> Reply {
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

// ── Catalog, attestors and workers ───────────────────────────────────────────────────────

/// Everything a purchase test needs: an organizer, a published event and an open sale.
pub struct Shop {
    pub organizer: String,
    pub event_id: String,
    pub sale_id: String,
    pub ticket_type_id: String,
}

impl TestApp {
    /// Creates an organizer, an organization, a published event and an open sale with one
    /// ticket type. `sale` may override fields of the sale request.
    pub async fn shop(&self, capacity: u32, price_minor: i64, sale: Value) -> Shop {
        let root = self.root_token();
        let organization = self
            .call(
                Method::POST,
                "/v1/admin/organizations",
                Some(&root),
                Some(json!({ "slug": format!("org-{}", uuid_suffix()), "name": "Circle" })),
            )
            .await;
        let organization_id = organization.body["id"].as_str().unwrap().to_owned();
        let email = format!("organizer-{}@example.org", uuid_suffix());
        let (_, account) = self.account("organizer", &email).await;
        let account_id = account["id"].as_str().unwrap();
        self.call(
            Method::PUT,
            &format!("/v1/admin/organizations/{organization_id}/members/{account_id}"),
            Some(&root),
            None,
        )
        .await;
        let (organizer, _) = self.login(&email).await;

        let now = self.now();
        let event = self
            .call(
                Method::POST,
                &format!("/v1/organizations/{organization_id}/events"),
                Some(&organizer),
                Some(json!({
                    "slug": format!("event-{}", uuid_suffix()),
                    "title": "Comic Market",
                    "venue": "Tokyo Big Sight",
                    "starts_at": (now + Duration::days(30)).to_string(),
                    "ends_at": (now + Duration::days(31)).to_string(),
                })),
            )
            .await;
        assert_eq!(event.status, StatusCode::CREATED, "{:?}", event.body);
        let event_id = event.body["id"].as_str().unwrap().to_owned();

        let mut sale_request = json!({
            "name": "General",
            "opens_at": (now - Duration::hours(1)).to_string(),
            "closes_at": (now + Duration::days(7)).to_string(),
            "reservation_ttl_seconds": 600,
            "max_tickets_per_request": 4,
        });
        if let (Some(base), Some(overrides)) = (sale_request.as_object_mut(), sale.as_object()) {
            base.extend(overrides.clone());
        }
        let sale = self
            .call(
                Method::POST,
                &format!("/v1/events/{event_id}/sales"),
                Some(&organizer),
                Some(sale_request),
            )
            .await;
        assert_eq!(sale.status, StatusCode::CREATED, "{:?}", sale.body);
        let sale_id = sale.body["id"].as_str().unwrap().to_owned();

        let ticket_type = self
            .call(
                Method::POST,
                &format!("/v1/sales/{sale_id}/ticket-types"),
                Some(&organizer),
                Some(json!({
                    "name": "Day 1",
                    "price": { "amount_minor": price_minor, "currency": "JPY" },
                    "capacity": capacity,
                    "per_account_limit": 2,
                    "valid_from": (now + Duration::days(30)).to_string(),
                    "valid_until": (now + Duration::days(31)).to_string(),
                    "ticket_extensions": { "128": "HALL-A" },
                })),
            )
            .await;
        assert_eq!(
            ticket_type.status,
            StatusCode::CREATED,
            "{:?}",
            ticket_type.body
        );
        let ticket_type_id = ticket_type.body["id"].as_str().unwrap().to_owned();

        let mut published = event.body.clone();
        published["status"] = json!("published");
        let published = self
            .call(
                Method::PUT,
                &format!("/v1/events/{event_id}"),
                Some(&organizer),
                Some(published),
            )
            .await;
        assert_eq!(published.status, StatusCode::OK, "{:?}", published.body);

        Shop {
            organizer,
            event_id,
            sale_id,
            ticket_type_id,
        }
    }

    /// Lets the sale accept an attestor.
    pub async fn accept_attestor(&self, shop: &Shop, attestor_id: &str) {
        let sale = self
            .call(
                Method::GET,
                &format!("/v1/sales/{}", shop.sale_id),
                None,
                None,
            )
            .await;
        let mut sale = sale.body;
        sale["accepted_attestors"] = json!([attestor_id]);
        let updated = self
            .call(
                Method::PUT,
                &format!("/v1/sales/{}", shop.sale_id),
                Some(&shop.organizer),
                Some(sale),
            )
            .await;
        assert_eq!(updated.status, StatusCode::OK, "{:?}", updated.body);
    }

    /// Registers an attestor and returns its id and signing key.
    pub async fn attestor(&self, environment: &str) -> Attestor {
        let key = SigningKey::from_bytes(&rand_bytes());
        let created = self
            .call(
                Method::POST,
                "/v1/admin/attestors",
                Some(&self.root_token()),
                Some(json!({
                    "name": format!("gateway-{environment}"),
                    "environment": environment,
                    "keys": [{ "key_id": "k1", "public_key": STANDARD.encode(key.verifying_key().as_bytes()) }],
                })),
            )
            .await;
        assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);
        Attestor {
            id: created.body["id"].as_str().unwrap().to_owned(),
            key,
        }
    }

    /// Calls an attestor endpoint with a signed request.
    pub async fn signed(
        &self,
        attestor: &Attestor,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Reply {
        let bytes = body.as_ref().map(Value::to_string).unwrap_or_default();
        let signature = kippu_core::modules::payments::signature::sign_request(
            &attestor.key,
            attestor.id.parse().unwrap(),
            "k1",
            method.as_str(),
            path,
            self.now().unix_seconds(),
            bytes.as_bytes(),
        );
        self.call_with(method, path, None, body, &[("kippu-signature", &signature)])
            .await
    }

    /// Reports a payment for a reservation.
    pub async fn pay(
        &self,
        attestor: &Attestor,
        attestation_id: &str,
        reservation: &Value,
    ) -> Reply {
        self.signed(
            attestor,
            Method::POST,
            "/v1/payment-attestations",
            Some(json!({
                "attestation_id": attestation_id,
                "outcome": "paid",
                "reservation_id": reservation["id"],
                "amount": reservation["total"],
            })),
        )
        .await
    }

    /// Runs a background task until it reports it is idle.
    pub async fn drain(&self, task: &str) {
        let task = self.app.task(task).unwrap();
        while task.run_once(self.app.state().clone()).await.unwrap()
            == kippu_core::Progress::MoreWork
        {}
    }

    /// Registers a buyer and returns their access token.
    pub async fn buyer(&self) -> String {
        let email = format!("buyer-{}@example.org", uuid_suffix());
        self.call(
            Method::POST,
            "/v1/auth/register",
            None,
            Some(json!({ "email": email, "password": "correct horse battery", "display_name": "Buyer" })),
        )
        .await;
        self.login(&email).await.0
    }

    /// Submits a purchase request and returns the response.
    pub async fn purchase(&self, buyer: &str, shop: &Shop, quantity: u32, key: &str) -> Reply {
        self.call_with(
            Method::POST,
            &format!("/v1/sales/{}/purchase-requests", shop.sale_id),
            Some(buyer),
            Some(json!({ "items": [{ "ticket_type_id": shop.ticket_type_id, "quantity": quantity }] })),
            &[("idempotency-key", key)],
        )
        .await
    }

    /// Buys and reserves: submits, runs the worker, returns the reservation.
    pub async fn reserve(&self, buyer: &str, shop: &Shop, quantity: u32) -> Value {
        let submitted = self.purchase(buyer, shop, quantity, &uuid_suffix()).await;
        assert_eq!(
            submitted.status,
            StatusCode::ACCEPTED,
            "{:?}",
            submitted.body
        );
        self.drain("purchases").await;
        let id = submitted.body["id"].as_str().unwrap();
        let request = self
            .call(
                Method::GET,
                &format!("/v1/purchase-requests/{id}"),
                Some(buyer),
                None,
            )
            .await;
        assert_eq!(request.body["status"], "reserved", "{:?}", request.body);
        let reservation_id = request.body["reservation_id"].as_str().unwrap();
        self.call(
            Method::GET,
            &format!("/v1/reservations/{reservation_id}"),
            Some(buyer),
            None,
        )
        .await
        .body
    }
}

pub struct Attestor {
    pub id: String,
    pub key: SigningKey,
}

pub fn uuid_suffix() -> String {
    uuid::Uuid::now_v7().simple().to_string()
}

fn rand_bytes() -> [u8; 32] {
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes).unwrap();
    bytes
}
