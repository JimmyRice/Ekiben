//! The inbox and the bus against a real NATS server with JetStream.
//!
//! Set `EKIBEN_TEST_NATS_URL` (e.g. `nats://localhost:4222`) to run these; without it they are
//! skipped. Each test uses its own stream prefix.
#![allow(clippy::unwrap_used, reason = "test: a failure is the test failing")]

use std::time::Duration;

use kippu_domain::outbox::IntegrationEvent;
use kippu_domain::purchase::{Basket, LineItem, PurchaseRequest, PurchaseStatus};
use kippu_domain::{AccountId, PurchaseRequestId, ReservationId, SaleId, TicketTypeId, Timestamp};
use kippu_nats::{NatsOptions, NatsQueue};
use kippu_store::{Enqueued, EventBus, OutboxRecord, PurchaseInbox};

async fn queue(ack_wait: Duration) -> Option<(NatsQueue, String, String)> {
    let url = std::env::var("EKIBEN_TEST_NATS_URL").ok()?;
    let prefix = format!("T{}", uuid::Uuid::now_v7().simple()).to_uppercase();
    let options = NatsOptions {
        prefix: prefix.clone(),
        ack_wait,
        ..NatsOptions::default()
    };
    Some((
        NatsQueue::connect(&url, options).await.unwrap(),
        url,
        prefix,
    ))
}

fn request() -> PurchaseRequest {
    let now = Timestamp::from_unix_seconds(1_800_000_000);
    PurchaseRequest {
        id: PurchaseRequestId::generate(),
        account_id: AccountId::generate(),
        sale_id: SaleId::generate(),
        basket: Basket::new(vec![LineItem {
            ticket_type_id: TicketTypeId::generate(),
            quantity: 2,
        }])
        .unwrap(),
        status: PurchaseStatus::Queued,
        created_at: now,
        updated_at: now,
    }
}

#[tokio::test]
async fn the_inbox_deduplicates_and_redelivers_until_acknowledged() {
    let Some((inbox, _, _)) = queue(Duration::from_secs(1)).await else {
        return;
    };
    let (first, second) = (request(), request());
    assert_eq!(inbox.enqueue(&first).await.unwrap(), Enqueued::Accepted);
    // A retry, with another basket: answered with the request first stored.
    let mut retry = first.clone();
    retry.basket = second.basket.clone();
    assert_eq!(
        inbox.enqueue(&retry).await.unwrap(),
        Enqueued::Duplicate(Some(first.clone()))
    );
    assert_eq!(inbox.enqueue(&second).await.unwrap(), Enqueued::Accepted);

    let delivered = inbox.receive(10).await.unwrap();
    let ids: Vec<_> = delivered
        .iter()
        .map(|delivery| delivery.request().id)
        .collect();
    assert_eq!(ids, vec![first.id, second.id], "the retry was dropped");
    assert_eq!(
        delivered[0].request(),
        &first,
        "requests survive the trip intact"
    );

    // Not acknowledged: back after the ack wait.
    drop(delivered);
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    let again = inbox.receive(10).await.unwrap();
    assert_eq!(again.len(), 2);
    for delivery in again {
        delivery.ack().await.unwrap();
    }
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    assert!(
        inbox.receive(10).await.unwrap().is_empty(),
        "acknowledged for good"
    );
    assert_eq!(
        inbox.enqueue(&first).await.unwrap(),
        Enqueued::Duplicate(None),
        "still deduplicated, no longer held"
    );
}

#[tokio::test]
async fn the_bus_resumes_where_it_stopped_and_drops_repeats() {
    let Some((bus, url, prefix)) = queue(Duration::from_secs(30)).await else {
        return;
    };
    assert_eq!(bus.last_published().await.unwrap(), 0);
    let record = |sequence| OutboxRecord {
        sequence,
        event: IntegrationEvent::ReservationExpired {
            reservation_id: ReservationId::generate(),
        },
        created_at: Timestamp::from_unix_seconds(1_800_000_000),
    };
    let (one, two, three) = (record(1), record(2), record(3));
    bus.publish(&one).await.unwrap();
    bus.publish(&two).await.unwrap();
    assert_eq!(bus.last_published().await.unwrap(), 2);
    // A second relay repeating what the first published changes nothing.
    bus.publish(&two).await.unwrap();
    bus.publish(&three).await.unwrap();
    bus.publish(&three).await.unwrap();
    assert_eq!(bus.last_published().await.unwrap(), 3);
    let jetstream = async_nats::jetstream::new(async_nats::connect(url).await.unwrap());
    let mut stream = jetstream
        .get_stream(format!("{prefix}_EVENTS"))
        .await
        .unwrap();
    assert_eq!(
        stream.info().await.unwrap().state.messages,
        3,
        "each event once"
    );
}
