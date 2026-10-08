//! How many database statements the busiest paths run. Counts come from sqlx's own
//! `sqlx::query` events, so they hold for every adapter; an N+1 shows up as a count that grows
//! with the data. Everything is measured in one test: the counter is process-wide.

mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use axum::http::{Method, StatusCode};
use serde_json::json;
use support::{Reply, Shop, TestApp, uuid_suffix};
use tracing::Subscriber;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::{Context, SubscriberExt as _};

/// Counts the statements sqlx runs, on any thread.
#[derive(Clone, Default)]
struct Statements(Arc<AtomicU32>);

impl<S: Subscriber> Layer<S> for Statements {
    fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
        if event.metadata().target() == "sqlx::query" {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }
}

impl Statements {
    /// The statements `work` runs.
    async fn of<T>(&self, work: impl Future<Output = T>) -> (T, u32) {
        self.0.store(0, Ordering::Relaxed);
        let output = work.await;
        (output, self.0.load(Ordering::Relaxed))
    }
}

/// A sale behind a waiting room with three ticket types, so per-type queries would show.
async fn shop(app: &TestApp) -> Shop {
    let shop = app
        .shop(
            100,
            1_000,
            json!({ "admission": { "mode": "waiting_room", "admit_per_tick": 100, "max_backlog": 1000 } }),
        )
        .await;
    let now = app.now();
    for name in ["Day 2", "Day 3"] {
        let created = app
            .call(
                Method::POST,
                &format!("/v1/sales/{}/ticket-types", shop.sale_id),
                Some(&shop.organizer),
                Some(json!({
                    "name": name,
                    "price": { "amount_minor": 1_000, "currency": "JPY" },
                    "capacity": 100,
                    "valid_from": (now + kippu_domain::Duration::days(30)).to_string(),
                    "valid_until": (now + kippu_domain::Duration::days(31)).to_string(),
                })),
            )
            .await;
        assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);
    }
    shop
}

/// Submits a purchase request of one ticket with an admission pass.
async fn purchase(app: &TestApp, shop: &Shop, buyer: &str, pass: &str) -> Reply {
    let reply = app
        .call_with(
            Method::POST,
            &format!("/v1/sales/{}/purchase-requests", shop.sale_id),
            Some(buyer),
            Some(json!({ "items": [{ "ticket_type_id": shop.ticket_type_id, "quantity": 1 }] })),
            &[
                ("idempotency-key", &uuid_suffix()),
                ("kippu-admission-pass", pass),
            ],
        )
        .await;
    assert_eq!(reply.status, StatusCode::ACCEPTED, "{:?}", reply.body);
    reply
}

#[tokio::test]
async fn hot_paths_run_a_fixed_number_of_statements() {
    let statements = Statements::default();
    tracing::subscriber::set_global_default(
        tracing_subscriber::registry().with(statements.clone()),
    )
    .unwrap();
    let app = TestApp::start().await;
    let shop = shop(&app).await;

    let sale_path = format!("/v1/sales/{}", shop.sale_id);
    let (offer, offer_statements) = statements
        .of(app.call(Method::GET, &sale_path, None, None))
        .await;
    assert_eq!(offer.body["ticket_types"].as_array().unwrap().len(), 3);

    let buyer = app.buyer().await;
    let place = app
        .call(
            Method::POST,
            &format!("/v1/sales/{}/waiting-room", shop.sale_id),
            Some(&buyer),
            None,
        )
        .await;
    app.drain("admission").await;
    let (admitted, admission_statements) = statements
        .of(app.call(
            Method::POST,
            &format!("/v1/sales/{}/admission", shop.sale_id),
            Some(&buyer),
            Some(json!({ "queue_ticket": place.body["queue_ticket"]["token"] })),
        ))
        .await;
    let pass = admitted.body["admission_pass"]["token"].as_str().unwrap();

    let (_, purchase_statements) = statements.of(purchase(&app, &shop, &buyer, pass)).await;
    let ((), one_request) = statements.of(app.drain("purchases")).await;
    for _ in 0..3 {
        purchase(&app, &shop, &buyer, pass).await;
    }
    let ((), three_requests) = statements.of(app.drain("purchases")).await;

    let counts = [
        ("sale offer", offer_statements, 4),
        ("admission", admission_statements, 3),
        ("purchase", purchase_statements, 4),
        ("worker, one request", one_request, 12),
        ("worker, three requests", three_requests, 30),
    ];
    for (path, measured, most) in counts {
        eprintln!("{path}: {measured} statements");
        assert!(
            measured <= most,
            "{path} runs {measured} statements, at most {most} expected"
        );
    }
}
