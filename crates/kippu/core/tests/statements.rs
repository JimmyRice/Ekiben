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

/// Counts the statements sqlx runs, on any thread, except those that set up a new pool
/// connection (SQLite's `PRAGMA`s, MySQL's session `SET`s). Opening a connection is the
/// pool's work, not the path's: a pool opens one whenever the connection a call just released
/// is still on its way back, which a busy machine makes likely at any moment.
#[derive(Clone, Default)]
struct Statements(Arc<AtomicU32>);

/// The first words of a statement, as sqlx's event summarizes it.
#[derive(Default)]
struct Summary(String);

impl tracing::field::Visit for Summary {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "summary" {
            value.clone_into(&mut self.0);
        }
    }

    fn record_debug(&mut self, _: &tracing::field::Field, _: &dyn std::fmt::Debug) {}
}

impl<S: Subscriber> Layer<S> for Statements {
    fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
        if event.metadata().target() != "sqlx::query" {
            return;
        }
        let mut summary = Summary::default();
        event.record(&mut summary);
        if !(summary.0.starts_with("PRAGMA ") || summary.0.starts_with("SET ")) {
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
    let waiting_room = format!("/v1/sales/{}/waiting-room", shop.sale_id);
    let (place, join_statements) = statements
        .of(app.call(Method::POST, &waiting_room, Some(&buyer), None))
        .await;
    let (_, rejoin_statements) = statements
        .of(app.call(Method::POST, &waiting_room, Some(&buyer), None))
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

    // Ten buyers in one batch, served together: what each request costs shows here.
    let mut tickets = Vec::new();
    for _ in 0..10 {
        let buyer = app.buyer().await;
        let place = app
            .call(Method::POST, &waiting_room, Some(&buyer), None)
            .await;
        tickets.push((buyer, place.body["queue_ticket"]["token"].clone()));
    }
    app.drain("admission").await;
    for (buyer, ticket) in &tickets {
        let admitted = app
            .call(
                Method::POST,
                &format!("/v1/sales/{}/admission", shop.sale_id),
                Some(buyer),
                Some(json!({ "queue_ticket": ticket })),
            )
            .await;
        let pass = admitted.body["admission_pass"]["token"].as_str().unwrap();
        purchase(&app, &shop, buyer, pass).await;
    }
    let ((), ten_buyers) = statements.of(app.drain("purchases")).await;

    // MySQL has neither RETURNING nor ON CONFLICT, so its writes take more statements, and it
    // enters a first join in a transaction of its own.
    let (join, worker_one, worker_three, worker_ten) =
        match std::env::var("EKIBEN_TEST_BACKEND").as_deref() {
            Ok("mysql") => (7, 13, 15, 15),
            _ => (5, 12, 13, 13),
        };
    let counts = [
        ("sale offer", offer_statements, 4),
        ("join waiting room", join_statements, join),
        ("join again", rejoin_statements, 3),
        ("admission", admission_statements, 3),
        ("purchase", purchase_statements, 3),
        ("worker, one request", one_request, worker_one),
        ("worker, three requests", three_requests, worker_three),
        ("worker, ten buyers", ten_buyers, worker_ten),
    ];
    for (path, measured, most) in counts {
        eprintln!("{path}: {measured} statements");
        assert!(
            measured <= most,
            "{path} runs {measured} statements, at most {most} expected"
        );
    }
}
