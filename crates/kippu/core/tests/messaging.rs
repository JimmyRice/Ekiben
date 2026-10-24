//! A queue in front of the database (the purchase inbox) and a bus for events, through the
//! ports any messaging adapter implements. NATS has its own tests in `kippu-nats`.
#![allow(
    clippy::unwrap_used,
    clippy::missing_panics_doc,
    reason = "test fakes and helpers: a failure is the test failing"
)]

mod support;

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::http::{Method, StatusCode};
use kippu_domain::purchase::PurchaseRequest;
use kippu_store::{Enqueued, EventBus, InboxDelivery, OutboxRecord, PurchaseInbox, StoreResult};
use serde_json::json;
use support::{Shop, TestApp, uuid_suffix};

/// An inbox that keeps requests in memory and drops ids it has seen, like JetStream's
/// deduplication: a duplicate is answered with the request held under its id until that one
/// is acknowledged.
#[derive(Default)]
struct MemoryInbox {
    queued: Mutex<VecDeque<PurchaseRequest>>,
    /// Every id ever enqueued, with the request while it is not acknowledged.
    seen: Arc<Mutex<HashMap<String, Option<PurchaseRequest>>>>,
    enqueued: Mutex<usize>,
}

struct Delivery {
    request: PurchaseRequest,
    seen: Arc<Mutex<HashMap<String, Option<PurchaseRequest>>>>,
}

#[async_trait]
impl InboxDelivery for Delivery {
    fn request(&self) -> &PurchaseRequest {
        &self.request
    }

    async fn ack(self: Box<Self>) -> StoreResult<()> {
        self.seen
            .lock()
            .unwrap()
            .insert(self.request.id.to_string(), None);
        Ok(())
    }
}

#[async_trait]
impl PurchaseInbox for MemoryInbox {
    async fn enqueue(&self, request: &PurchaseRequest) -> StoreResult<Enqueued> {
        *self.enqueued.lock().unwrap() += 1;
        let mut seen = self.seen.lock().unwrap();
        if let Some(held) = seen.get(&request.id.to_string()) {
            return Ok(Enqueued::Duplicate(held.clone()));
        }
        seen.insert(request.id.to_string(), Some(request.clone()));
        self.queued.lock().unwrap().push_back(request.clone());
        Ok(Enqueued::Accepted)
    }

    async fn receive(&self, limit: usize) -> StoreResult<Vec<Box<dyn InboxDelivery>>> {
        let mut queued = self.queued.lock().unwrap();
        let count = limit.min(queued.len());
        Ok(queued
            .drain(..count)
            .map(|request| {
                Box::new(Delivery {
                    request,
                    seen: self.seen.clone(),
                }) as Box<dyn InboxDelivery>
            })
            .collect())
    }
}

/// A bus that keeps events in memory and drops sequences it already has.
#[derive(Default)]
struct MemoryBus {
    published: Mutex<Vec<OutboxRecord>>,
}

#[async_trait]
impl EventBus for MemoryBus {
    async fn last_published(&self) -> StoreResult<i64> {
        Ok(self
            .published
            .lock()
            .unwrap()
            .last()
            .map_or(0, |record| record.sequence))
    }

    async fn publish(&self, record: &OutboxRecord) -> StoreResult<()> {
        let mut published = self.published.lock().unwrap();
        if published
            .iter()
            .all(|seen| seen.sequence != record.sequence)
        {
            published.push(record.clone());
        }
        Ok(())
    }
}

async fn app(inbox: Arc<MemoryInbox>, bus: Arc<MemoryBus>) -> TestApp {
    TestApp::start_custom(
        Vec::new(),
        |_| {},
        move |kippu| kippu.inbox(inbox).event_bus(bus),
    )
    .await
}

async fn submit(app: &TestApp, buyer: &str, shop: &Shop, key: &str) -> support::Reply {
    app.purchase(buyer, shop, 1, key).await
}

#[tokio::test]
async fn queued_requests_are_answered_by_receipt_until_persisted() {
    let inbox = Arc::new(MemoryInbox::default());
    let app = app(inbox.clone(), Arc::new(MemoryBus::default())).await;
    let shop = app.shop(10, 1_000, json!({})).await;
    let buyer = app.buyer().await;

    let accepted = submit(&app, &buyer, &shop, "inbox-1").await;
    assert_eq!(accepted.status, StatusCode::ACCEPTED, "{:?}", accepted.body);
    let location = accepted.headers["location"].to_str().unwrap().to_owned();
    assert!(location.contains("?receipt="), "{location}");
    let id = accepted.body["id"].as_str().unwrap();
    let bare = format!("/v1/purchase-requests/{id}");

    // Not in the database yet: the receipt answers, nothing else does.
    let polled = app.call(Method::GET, &location, Some(&buyer), None).await;
    assert_eq!(polled.status, StatusCode::OK);
    assert_eq!(polled.body["status"], "queued");
    assert_eq!(polled.body["basket"], accepted.body["basket"]);
    assert_eq!(
        app.call(Method::GET, &bare, Some(&buyer), None)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    let stranger = app.buyer().await;
    assert_eq!(
        app.call(Method::GET, &location, Some(&stranger), None)
            .await
            .status,
        StatusCode::NOT_FOUND,
        "a receipt is only good for its owner"
    );

    // A retry while it is in the inbox is deduplicated there and answered with the first
    // attempt; one with another basket is refused, as the database would refuse it.
    let retried = submit(&app, &buyer, &shop, "inbox-1").await;
    assert_eq!(retried.status, StatusCode::ACCEPTED, "{:?}", retried.body);
    assert_eq!(retried.body, accepted.body);
    assert_eq!(inbox.queued.lock().unwrap().len(), 1);
    let other_basket = app
        .call_with(
            Method::POST,
            &format!("/v1/sales/{}/purchase-requests", shop.sale_id),
            Some(&buyer),
            Some(json!({ "items": [{ "ticket_type_id": shop.ticket_type_id, "quantity": 2 }] })),
            &[("idempotency-key", "inbox-1")],
        )
        .await;
    assert_eq!(
        other_basket.body["type"], "urn:kippu:problem:idempotency-key-reused",
        "{:?}",
        other_basket.body
    );
    assert_eq!(inbox.queued.lock().unwrap().len(), 1);

    app.drain("purchase-inbox").await;
    assert_eq!(
        app.call(Method::GET, &bare, Some(&buyer), None).await.body["status"],
        "queued",
        "persisted"
    );
    app.drain("purchases").await;
    let reserved = app.call(Method::GET, &location, Some(&buyer), None).await;
    assert_eq!(reserved.body["status"], "reserved", "{:?}", reserved.body);

    // Once persisted, a retry is answered from the database without touching the inbox.
    let before = *inbox.enqueued.lock().unwrap();
    let retried = submit(&app, &buyer, &shop, "inbox-1").await;
    assert_eq!(retried.body["status"], "reserved");
    assert_eq!(*inbox.enqueued.lock().unwrap(), before);
    let other_basket = app
        .call_with(
            Method::POST,
            &format!("/v1/sales/{}/purchase-requests", shop.sale_id),
            Some(&buyer),
            Some(json!({ "items": [{ "ticket_type_id": shop.ticket_type_id, "quantity": 2 }] })),
            &[("idempotency-key", "inbox-1")],
        )
        .await;
    assert_eq!(
        other_basket.body["type"],
        "urn:kippu:problem:idempotency-key-reused"
    );
}

#[tokio::test]
async fn a_duplicate_the_inbox_no_longer_holds_is_answered_from_the_database() {
    let inbox = Arc::new(MemoryInbox::default());
    let app = app(inbox.clone(), Arc::new(MemoryBus::default())).await;
    let shop = app.shop(10, 1_000, json!({})).await;
    let buyer = app.buyer().await;
    let accepted = submit(&app, &buyer, &shop, "gone").await;
    let id = accepted.body["id"].as_str().unwrap().to_owned();

    // Acknowledged but, as seen by this retry, not in the database yet: never guess.
    inbox.seen.lock().unwrap().insert(id.clone(), None);
    let retried = submit(&app, &buyer, &shop, "gone").await;
    assert_eq!(
        retried.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{:?}",
        retried.body
    );

    app.drain("purchase-inbox").await;
    let retried = submit(&app, &buyer, &shop, "gone").await;
    assert_eq!(retried.status, StatusCode::ACCEPTED, "{:?}", retried.body);
    assert_eq!(retried.body["id"], id.as_str());
}

#[tokio::test]
async fn first_come_first_served_holds_through_the_inbox() {
    let app = app(
        Arc::new(MemoryInbox::default()),
        Arc::new(MemoryBus::default()),
    )
    .await;
    let shop = app.shop(1, 1_000, json!({})).await;
    let (first, second) = (app.buyer().await, app.buyer().await);
    let early = submit(&app, &first, &shop, &uuid_suffix()).await;
    app.clock.advance(kippu_domain::Duration::milliseconds(1));
    let late = submit(&app, &second, &shop, &uuid_suffix()).await;
    app.drain("purchase-inbox").await;
    app.drain("purchases").await;
    for (buyer, reply, expected) in [(&first, &early, "reserved"), (&second, &late, "rejected")] {
        let location = reply.headers["location"].to_str().unwrap();
        let polled = app.call(Method::GET, location, Some(buyer), None).await;
        assert_eq!(polled.body["status"], expected, "{:?}", polled.body);
    }
}

#[tokio::test]
async fn outbox_events_are_relayed_once_and_in_order() {
    let bus = Arc::new(MemoryBus::default());
    let app = app(Arc::new(MemoryInbox::default()), bus.clone()).await;
    let shop = app.shop(10, 0, json!({})).await;
    for _ in 0..3 {
        let buyer = app.buyer().await;
        submit(&app, &buyer, &shop, &uuid_suffix()).await;
    }
    app.drain("purchase-inbox").await;
    app.drain("purchases").await;
    // Free reservations are issued at checkout: one `tickets.issued` each.
    let reservations = app.app.state().store().outbox_after(0, 100).await.unwrap();
    assert!(
        reservations.is_empty(),
        "nothing happened that other systems care about yet"
    );

    let buyer = app.buyer().await;
    let reservation = app.reserve(&buyer, &shop, 1).await;
    let path = format!(
        "/v1/reservations/{}/checkout",
        reservation["id"].as_str().unwrap()
    );
    app.call(Method::POST, &path, Some(&buyer), Some(json!({})))
        .await;

    app.drain("event-relay").await;
    app.drain("event-relay").await;
    let published = bus.published.lock().unwrap().clone();
    let outbox = app.app.state().store().outbox_after(0, 100).await.unwrap();
    assert_ne!(outbox.len(), 0, "the checkout recorded events");
    assert_eq!(published, outbox, "every event, once, in order");
}
