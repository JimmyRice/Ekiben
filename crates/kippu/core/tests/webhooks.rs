//! Webhooks end to end: events reach a real HTTP receiver, signed, in order, scoped to the
//! organization, and survive a failing receiver.
#![allow(
    clippy::unwrap_used,
    clippy::missing_panics_doc,
    reason = "test helpers: a failure is the test failing"
)]

mod support;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use ed25519_dalek::VerifyingKey;
use kippu_core::modules::webhooks::signature;
use kippu_domain::Duration;
use serde_json::{Value, json};
use support::{Shop, TestApp};

/// What the receiver saw: every attempt, and whether it accepted it.
#[derive(Default)]
struct Inbox {
    attempts: Mutex<Vec<(HeaderMap, Bytes, bool)>>,
    failing: AtomicBool,
}

impl Inbox {
    fn accepted(&self) -> Vec<(HeaderMap, Value)> {
        self.attempts
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, _, accepted)| *accepted)
            .map(|(headers, body, _)| (headers.clone(), serde_json::from_slice(body).unwrap()))
            .collect()
    }

    fn attempt_count(&self) -> usize {
        self.attempts.lock().unwrap().len()
    }
}

async fn receive(State(inbox): State<Arc<Inbox>>, headers: HeaderMap, body: Bytes) -> StatusCode {
    let accept = !inbox.failing.load(Ordering::SeqCst);
    inbox.attempts.lock().unwrap().push((headers, body, accept));
    if accept {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

/// A receiver on a local port; returns its URL.
async fn receiver(inbox: Arc<Inbox>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let router = axum::Router::new()
        .route("/hooks/kippu", axum::routing::post(receive))
        .with_state(inbox);
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    format!("http://{address}/hooks/kippu")
}

/// An instance with webhooks enabled, allowed to reach the local receiver.
async fn app() -> TestApp {
    TestApp::start_with(Vec::new(), |config| {
        config.keys.webhook_signing_key = Some(STANDARD.encode([3; 32]).into());
        config.webhooks.allow_http = true;
        config.webhooks.allow_private_networks = true;
    })
    .await
}

async fn organization_of(app: &TestApp, shop: &Shop) -> String {
    let event = app
        .call(
            Method::GET,
            &format!("/v1/events/{}", shop.event_id),
            None,
            None,
        )
        .await;
    event.body["organization_id"].as_str().unwrap().to_owned()
}

/// A free ticket for a new buyer: `tickets.issued` for the shop's organization.
async fn free_ticket(app: &TestApp, shop: &Shop) {
    let buyer = app.buyer().await;
    let reservation = app.reserve(&buyer, shop, 1).await;
    let path = format!(
        "/v1/reservations/{}/checkout",
        reservation["id"].as_str().unwrap()
    );
    let checkout = app
        .call(Method::POST, &path, Some(&buyer), Some(json!({})))
        .await;
    assert_eq!(checkout.body["status"], "issued", "{:?}", checkout.body);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_organization_receives_its_own_events_signed_and_in_order() {
    let app = app().await;
    let inbox = Arc::new(Inbox::default());
    let url = receiver(inbox.clone()).await;
    let ours = app.shop(10, 0, json!({})).await;
    let theirs = app.shop(10, 0, json!({})).await;
    let organization = organization_of(&app, &ours).await;

    let created = app
        .call(
            Method::POST,
            &format!("/v1/organizations/{organization}/webhooks"),
            Some(&ours.organizer),
            Some(json!({ "url": url, "topics": ["tickets.issued"] })),
        )
        .await;
    assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);
    let webhook_id = created.body["id"].as_str().unwrap().to_owned();

    free_ticket(&app, &ours).await;
    free_ticket(&app, &theirs).await;
    free_ticket(&app, &ours).await;
    app.drain("webhooks").await;

    let accepted = inbox.accepted();
    assert_eq!(accepted.len(), 2, "only our organization's events");
    let keys = app
        .call(Method::GET, "/.well-known/kippu/webhook-keys", None, None)
        .await;
    let public: [u8; 32] = STANDARD
        .decode(keys.body["keys"][0]["public_key"].as_str().unwrap())
        .unwrap()
        .try_into()
        .unwrap();
    let public = VerifyingKey::from_bytes(&public).unwrap();
    let attempts = inbox.attempts.lock().unwrap().clone();
    let mut sequences = Vec::new();
    for (headers, body, _) in &attempts {
        let header = headers["kippu-signature"].to_str().unwrap();
        assert!(
            signature::verify(&public, header, &url, body).is_some(),
            "signature checks out"
        );
        let body: Value = serde_json::from_slice(body).unwrap();
        assert_eq!(body["event"]["topic"], "tickets.issued");
        assert_eq!(body["webhook_id"], webhook_id);
        let sequence = body["sequence"].as_i64().unwrap();
        assert_eq!(
            headers["kippu-delivery"].to_str().unwrap(),
            format!("{webhook_id}:{sequence}")
        );
        sequences.push(sequence);
    }
    assert!(
        sequences.windows(2).all(|pair| pair[0] < pair[1]),
        "in order"
    );

    let progress = app
        .call(
            Method::GET,
            &format!("/v1/webhooks/{webhook_id}"),
            Some(&ours.organizer),
            None,
        )
        .await;
    assert_eq!(progress.body["failures"], 0);
    assert!(progress.body["delivered_through"].as_i64().unwrap() >= sequences[1]);

    // Other organizers cannot see it.
    let hidden = app
        .call(
            Method::GET,
            &format!("/v1/webhooks/{webhook_id}"),
            Some(&theirs.organizer),
            None,
        )
        .await;
    assert_eq!(hidden.status, StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failing_receiver_is_retried_later_without_losing_events() {
    let app = app().await;
    let inbox = Arc::new(Inbox::default());
    let url = receiver(inbox.clone()).await;
    let shop = app.shop(10, 0, json!({})).await;

    let created = app
        .call(
            Method::POST,
            "/v1/admin/webhooks",
            Some(&app.root_token()),
            Some(json!({ "url": url })),
        )
        .await;
    assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);
    let path = format!("/v1/webhooks/{}", created.body["id"].as_str().unwrap());

    inbox.failing.store(true, Ordering::SeqCst);
    free_ticket(&app, &shop).await;
    app.drain("webhooks").await;
    assert_eq!(inbox.attempt_count(), 1, "the first failure ends the run");
    let failed = app
        .call(Method::GET, &path, Some(&app.root_token()), None)
        .await;
    assert_eq!(failed.body["failures"], 1);
    assert!(
        failed.body["last_error"]
            .as_str()
            .unwrap()
            .contains("HTTP 500"),
        "{:?}",
        failed.body
    );

    // Not due yet: nothing is attempted.
    app.drain("webhooks").await;
    assert_eq!(inbox.attempt_count(), 1);

    inbox.failing.store(false, Ordering::SeqCst);
    app.clock.advance(Duration::seconds(3));
    app.drain("webhooks").await;
    let accepted = inbox.accepted();
    assert!(
        !accepted.is_empty(),
        "delivered once the receiver recovered"
    );
    assert!(
        accepted
            .iter()
            .any(|(_, body)| body["event"]["topic"] == "tickets.issued")
    );
    let recovered = app
        .call(Method::GET, &path, Some(&app.root_token()), None)
        .await;
    assert_eq!(recovered.body["failures"], 0);
    assert_eq!(recovered.body["last_error"], Value::Null);
}

#[tokio::test]
async fn urls_are_checked_against_the_deployment_policy() {
    let strict = TestApp::start_with(Vec::new(), |config| {
        config.keys.webhook_signing_key = Some(STANDARD.encode([3; 32]).into());
    })
    .await;
    for url in [
        "http://hooks.example.org/kippu",
        "https://127.0.0.1/kippu",
        "https://169.254.169.254/latest/meta-data",
    ] {
        let refused = strict
            .call(
                Method::POST,
                "/v1/admin/webhooks",
                Some(&strict.root_token()),
                Some(json!({ "url": url })),
            )
            .await;
        assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY, "{url}");
    }
    let unknown_topic = strict
        .call(
            Method::POST,
            "/v1/admin/webhooks",
            Some(&strict.root_token()),
            Some(json!({ "url": "https://hooks.example.org/kippu", "topics": ["tickets.sold"] })),
        )
        .await;
    assert_eq!(unknown_topic.status, StatusCode::UNPROCESSABLE_ENTITY);

    let unconfigured = TestApp::start().await;
    let refused = unconfigured
        .call(
            Method::POST,
            "/v1/admin/webhooks",
            Some(&unconfigured.root_token()),
            Some(json!({ "url": "https://hooks.example.org/kippu" })),
        )
        .await;
    assert_eq!(refused.status, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(
        refused.body["type"],
        "urn:kippu:problem:webhooks-not-configured"
    );
}
