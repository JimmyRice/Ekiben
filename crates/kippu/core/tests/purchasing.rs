//! The purchase pipeline through the HTTP API: contention, idempotency, payment and refunds.

mod support;

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::http::{Method, StatusCode};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use kippu_domain::Duration;
use serde_json::{Value, json};
use support::{TestApp, uuid_suffix};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn many_concurrent_buyers_never_oversell() {
    let app = Arc::new(TestApp::start().await);
    let shop = Arc::new(app.shop(20, 1_000, json!({})).await);

    let mut buyers = Vec::new();
    for _ in 0..60 {
        buyers.push(app.buyer().await);
    }
    // Everyone submits at once.
    let submissions: Vec<_> = buyers
        .iter()
        .map(|buyer| {
            let (app, shop, buyer) = (app.clone(), shop.clone(), buyer.clone());
            tokio::spawn(async move { app.purchase(&buyer, &shop, 1, "only-once").await })
        })
        .collect();
    let mut replies = Vec::new();
    for submission in submissions {
        let reply = submission.await.unwrap();
        assert_eq!(reply.status, StatusCode::ACCEPTED);
        replies.push(reply);
    }

    app.drain("purchases").await;

    let mut outcomes: BTreeMap<String, usize> = BTreeMap::new();
    for (buyer, submission) in buyers.iter().zip(&replies) {
        let id = submission.body["id"].as_str().unwrap();
        let request = app
            .call(
                Method::GET,
                &format!("/v1/purchase-requests/{id}"),
                Some(buyer),
                None,
            )
            .await;
        let outcome = match request.body["status"].as_str().unwrap() {
            "rejected" => format!("rejected:{}", request.body["reason"].as_str().unwrap()),
            status => status.to_owned(),
        };
        *outcomes.entry(outcome).or_default() += 1;
    }
    assert_eq!(outcomes.get("reserved"), Some(&20));
    assert_eq!(outcomes.get("rejected:sold_out"), Some(&40));

    let sale = app
        .call(
            Method::GET,
            &format!("/v1/sales/{}", shop.sale_id),
            None,
            None,
        )
        .await;
    assert_eq!(sale.body["ticket_types"][0]["available"], 0);
}

#[tokio::test]
async fn retrying_a_purchase_is_the_same_purchase() {
    let app = TestApp::start().await;
    let shop = app.shop(10, 1_000, json!({})).await;
    let buyer = app.buyer().await;

    let first = app.purchase(&buyer, &shop, 1, "retry-key").await;
    for _ in 0..9 {
        let retry = app.purchase(&buyer, &shop, 1, "retry-key").await;
        assert_eq!(retry.status, StatusCode::ACCEPTED);
        assert_eq!(retry.body["id"], first.body["id"]);
    }
    assert!(
        first.headers["location"]
            .to_str()
            .unwrap()
            .ends_with(first.body["id"].as_str().unwrap())
    );

    let different = app.purchase(&buyer, &shop, 2, "retry-key").await;
    assert_eq!(different.status, StatusCode::UNPROCESSABLE_ENTITY);

    let missing_key = app
        .call(
            Method::POST,
            &format!("/v1/sales/{}/purchase-requests", shop.sale_id),
            Some(&buyer),
            Some(json!({ "items": [{ "ticket_type_id": shop.ticket_type_id, "quantity": 1 }] })),
        )
        .await;
    assert_eq!(missing_key.status, StatusCode::UNPROCESSABLE_ENTITY);

    app.drain("purchases").await;
    let reservations = app
        .call(Method::GET, "/v1/me/reservations", Some(&buyer), None)
        .await;
    assert_eq!(
        reservations.body["items"].as_array().unwrap().len(),
        1,
        "ten submissions, one reservation"
    );
}

#[tokio::test]
async fn an_attested_payment_issues_verifiable_tickets_exactly_once() {
    let app = TestApp::start().await;
    let shop = app.shop(10, 3_000, json!({})).await;
    let attestor = app.attestor("live").await;
    app.accept_attestor(&shop, &attestor.id).await;
    let buyer = app.buyer().await;
    let reservation = app.reserve(&buyer, &shop, 2).await;
    assert_eq!(
        reservation["total"],
        json!({ "amount_minor": 6000, "currency": "JPY" })
    );

    let checkout = app
        .call(
            Method::POST,
            &format!(
                "/v1/reservations/{}/checkout",
                reservation["id"].as_str().unwrap()
            ),
            Some(&buyer),
            Some(json!({ "attestor_id": attestor.id })),
        )
        .await;
    assert_eq!(checkout.body["status"], "payment_pending");
    let feed = app
        .signed(&attestor, Method::GET, "/v1/attestor/feed", None)
        .await;
    assert_eq!(feed.body[0]["event"]["topic"], "payment.requested");

    let paid = app.pay(&attestor, "pi_1", &reservation).await;
    assert_eq!(paid.status, StatusCode::OK, "{:?}", paid.body);
    assert_eq!(paid.body["disposition"], "applied");
    assert_eq!(paid.body["reservation"]["status"], "issued");
    assert_eq!(paid.body["ticket_ids"].as_array().unwrap().len(), 2);

    for _ in 0..9 {
        let again = app.pay(&attestor, "pi_1", &reservation).await;
        assert_eq!(again.body["replayed"], true);
        assert_eq!(again.body["ticket_ids"], paid.body["ticket_ids"]);
    }

    let tickets = app
        .call(Method::GET, "/v1/me/tickets", Some(&buyer), None)
        .await;
    let tickets = tickets.body["items"].as_array().unwrap();
    assert_eq!(tickets.len(), 2, "ten deliveries, one set of tickets");

    // Verify like a gate would: trusted keys from the well-known endpoint, then Kaisatsu.
    let keys = app
        .call(Method::GET, "/.well-known/kippu/ticket-keys", None, None)
        .await;
    let public_key: [u8; 32] = STANDARD
        .decode(keys.body["keys"][0]["public_key"].as_str().unwrap())
        .unwrap()
        .try_into()
        .unwrap();
    let gate = kaisatsu::Verifier::new(kaisatsu::TrustedKey::from_bytes(&public_key).unwrap());
    let bytes = STANDARD
        .decode(tickets[0]["ticket"].as_str().unwrap())
        .unwrap();
    let ticket = gate.verify(&bytes).unwrap();
    assert_eq!(ticket.issuer(), support::ISSUER);
    assert_eq!(
        ticket.ticket_id().to_string(),
        tickets[0]["id"].as_str().unwrap()
    );
    assert_eq!(ticket.event_id().to_string(), shop.event_id);
    assert_eq!(ticket.extensions().get(0x80), Some(&b"HALL-A"[..]));
}

async fn expire_everything(app: &TestApp) {
    app.clock.advance(Duration::minutes(11));
    app.drain("reservation-expiry").await;
}

#[tokio::test]
async fn a_late_payment_reacquires_stock_when_some_is_left() {
    let app = TestApp::start().await;
    let shop = app.shop(10, 1_000, json!({})).await;
    let attestor = app.attestor("live").await;
    app.accept_attestor(&shop, &attestor.id).await;
    let buyer = app.buyer().await;
    let reservation = app.reserve(&buyer, &shop, 1).await;

    expire_everything(&app).await;
    let expired = app
        .call(
            Method::GET,
            &format!("/v1/reservations/{}", reservation["id"].as_str().unwrap()),
            Some(&buyer),
            None,
        )
        .await;
    assert_eq!(expired.body["status"], "expired");

    let paid = app.pay(&attestor, "late", &reservation).await;
    assert_eq!(paid.body["disposition"], "applied");
    assert_eq!(paid.body["reservation"]["status"], "issued");
}

#[tokio::test]
async fn a_late_payment_for_sold_out_stock_requires_a_refund() {
    let app = TestApp::start().await;
    let shop = app.shop(1, 1_000, json!({})).await;
    let attestor = app.attestor("live").await;
    app.accept_attestor(&shop, &attestor.id).await;
    let slow = app.buyer().await;
    let reservation = app.reserve(&slow, &shop, 1).await;
    expire_everything(&app).await;

    let fast = app.buyer().await;
    app.reserve(&fast, &shop, 1).await; // takes the last ticket

    let paid = app.pay(&attestor, "too-late", &reservation).await;
    assert_eq!(
        paid.status,
        StatusCode::OK,
        "recorded: the attestor must stop retrying"
    );
    assert_eq!(paid.body["disposition"], "refund_required");
    assert_eq!(paid.body["reservation"]["status"], "refund_required");

    let feed = app
        .signed(&attestor, Method::GET, "/v1/attestor/feed", None)
        .await;
    let refund = feed
        .body
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["event"]["topic"] == "refund.required")
        .unwrap();
    assert_eq!(refund["event"]["attestation_id"], "too-late");

    let refunded = app
        .signed(
            &attestor,
            Method::POST,
            "/v1/payment-attestations",
            Some(json!({ "attestation_id": "too-late", "outcome": "refunded" })),
        )
        .await;
    assert_eq!(refunded.body["disposition"], "refunded");
    assert_eq!(refunded.body["reservation"]["status"], "refunded");
}

#[tokio::test]
async fn a_second_payment_for_the_same_reservation_is_refunded() {
    let app = TestApp::start().await;
    let shop = app.shop(10, 1_000, json!({})).await;
    let attestor = app.attestor("live").await;
    app.accept_attestor(&shop, &attestor.id).await;
    let buyer = app.buyer().await;
    let reservation = app.reserve(&buyer, &shop, 1).await;

    assert_eq!(
        app.pay(&attestor, "first", &reservation).await.body["disposition"],
        "applied"
    );
    let second = app.pay(&attestor, "second", &reservation).await;
    assert_eq!(second.body["disposition"], "refund_required");
    assert_eq!(
        second.body["reservation"]["status"], "issued",
        "the tickets stay issued"
    );
    assert_eq!(
        app.call(Method::GET, "/v1/me/tickets", Some(&buyer), None)
            .await
            .body["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn untrusted_payments_are_refused() {
    let app = TestApp::start().await;
    let shop = app.shop(10, 1_000, json!({})).await;
    let live = app.attestor("live").await;
    let sandbox = app.attestor("sandbox").await;
    let stranger = app.attestor("live").await;
    let sale = app
        .call(
            Method::GET,
            &format!("/v1/sales/{}", shop.sale_id),
            None,
            None,
        )
        .await;
    let mut sale = sale.body;
    sale["accepted_attestors"] = json!([live.id, sandbox.id]);
    app.call(
        Method::PUT,
        &format!("/v1/sales/{}", shop.sale_id),
        Some(&shop.organizer),
        Some(sale),
    )
    .await;
    let buyer = app.buyer().await;
    let reservation = app.reserve(&buyer, &shop, 1).await;

    let sandboxed = app.pay(&sandbox, "s", &reservation).await;
    assert_eq!(
        sandboxed.body["type"],
        "urn:kippu:problem:environment-mismatch"
    );
    let not_accepted = app.pay(&stranger, "x", &reservation).await;
    assert_eq!(
        not_accepted.body["type"],
        "urn:kippu:problem:attestor-not-accepted"
    );

    let mut wrong_amount = reservation.clone();
    wrong_amount["total"]["amount_minor"] = json!(1);
    assert_eq!(
        app.pay(&live, "cheap", &wrong_amount).await.body["type"],
        "urn:kippu:problem:amount-mismatch"
    );

    let forged = app
        .call_with(
            Method::POST,
            "/v1/payment-attestations",
            None,
            Some(json!({ "attestation_id": "f", "outcome": "paid" })),
            &[(
                "kippu-signature",
                &format!("attestor={}, key=k1, ts=0, sig=AAAA", live.id),
            )],
        )
        .await;
    assert_eq!(forged.status, StatusCode::UNAUTHORIZED);

    app.call(
        Method::PUT,
        &format!("/v1/admin/attestors/{}/revoked", live.id),
        Some(&app.root_token()),
        Some(json!({ "revoked": true })),
    )
    .await;
    assert_eq!(
        app.pay(&live, "after-revocation", &reservation)
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn the_waiting_room_admits_in_batches() {
    let app = TestApp::start().await;
    let shop = app.shop(10, 1_000, json!({ "admission": { "mode": "waiting_room", "admit_per_tick": 2, "max_backlog": 100 } })).await;
    let mut places = Vec::new();
    for _ in 0..3 {
        let buyer = app.buyer().await;
        let place = app
            .call(
                Method::POST,
                &format!("/v1/sales/{}/waiting-room", shop.sale_id),
                Some(&buyer),
                None,
            )
            .await;
        places.push((buyer, place.body));
    }
    assert_eq!(
        places
            .iter()
            .map(|(_, place)| place["position"].clone())
            .collect::<Vec<_>>(),
        [json!(1), json!(2), json!(3)]
    );

    let sale_id = shop.sale_id.clone();
    let ask = async |buyer: &str, place: &Value| {
        app.call(
            Method::POST,
            &format!("/v1/sales/{sale_id}/admission"),
            Some(buyer),
            Some(json!({ "queue_ticket": place["queue_ticket"]["token"] })),
        )
        .await
    };
    assert_eq!(
        ask(&places[0].0, &places[0].1).await.body["status"],
        "waiting"
    );
    let no_pass = app.purchase(&places[0].0, &shop, 1, "k").await;
    assert_eq!(no_pass.status, StatusCode::FORBIDDEN);

    app.drain("admission").await;
    let admitted = ask(&places[0].0, &places[0].1).await;
    assert_eq!(admitted.body["status"], "admitted");
    assert_eq!(
        ask(&places[1].0, &places[1].1).await.body["status"],
        "admitted"
    );
    assert_eq!(
        ask(&places[2].0, &places[2].1).await.body["status"],
        "waiting"
    );

    let pass = admitted.body["admission_pass"]["token"].as_str().unwrap();
    let with_pass = app
        .call_with(
            Method::POST,
            &format!("/v1/sales/{}/purchase-requests", shop.sale_id),
            Some(&places[0].0),
            Some(json!({ "items": [{ "ticket_type_id": shop.ticket_type_id, "quantity": 1 }] })),
            &[("idempotency-key", "k"), ("kippu-admission-pass", pass)],
        )
        .await;
    assert_eq!(with_pass.status, StatusCode::ACCEPTED);
    let stolen_pass = app
        .call_with(
            Method::POST,
            &format!("/v1/sales/{}/purchase-requests", shop.sale_id),
            Some(&places[2].0),
            Some(json!({ "items": [{ "ticket_type_id": shop.ticket_type_id, "quantity": 1 }] })),
            &[("idempotency-key", "k"), ("kippu-admission-pass", pass)],
        )
        .await;
    assert_eq!(
        stolen_pass.status,
        StatusCode::FORBIDDEN,
        "passes are bound to their holder"
    );
}

#[tokio::test]
async fn free_tickets_are_issued_at_checkout() {
    let app = TestApp::start().await;
    let shop = app.shop(10, 0, json!({})).await;
    let buyer = app.buyer().await;
    let reservation = app.reserve(&buyer, &shop, 1).await;
    let checkout = app
        .call(
            Method::POST,
            &format!(
                "/v1/reservations/{}/checkout",
                reservation["id"].as_str().unwrap()
            ),
            Some(&buyer),
            Some(json!({})),
        )
        .await;
    assert_eq!(checkout.body["status"], "issued", "{:?}", checkout.body);
    assert_eq!(
        app.call(Method::GET, "/v1/me/tickets", Some(&buyer), None)
            .await
            .body["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn limits_and_cancellation_are_enforced() {
    let app = TestApp::start().await;
    let shop = app.shop(10, 1_000, json!({})).await;
    let buyer = app.buyer().await;

    let too_many = app.purchase(&buyer, &shop, 3, &uuid_suffix()).await; // per-account limit is 2
    app.drain("purchases").await;
    let id = too_many.body["id"].as_str().unwrap();
    let rejected = app
        .call(
            Method::GET,
            &format!("/v1/purchase-requests/{id}"),
            Some(&buyer),
            None,
        )
        .await;
    assert_eq!(rejected.body["reason"], "limit_exceeded");

    let reservation = app.reserve(&buyer, &shop, 2).await;
    let reservation_id = reservation["id"].as_str().unwrap();
    let other = app.buyer().await;
    let peek = app
        .call(
            Method::GET,
            &format!("/v1/reservations/{reservation_id}"),
            Some(&other),
            None,
        )
        .await;
    assert_eq!(
        peek.status,
        StatusCode::NOT_FOUND,
        "other people's reservations are invisible"
    );

    let cancelled = app
        .call(
            Method::POST,
            &format!("/v1/reservations/{reservation_id}/cancel"),
            Some(&buyer),
            None,
        )
        .await;
    assert_eq!(cancelled.body["status"], "cancelled");
    let sale = app
        .call(
            Method::GET,
            &format!("/v1/sales/{}", shop.sale_id),
            None,
            None,
        )
        .await;
    assert_eq!(
        sale.body["ticket_types"][0]["available"], 10,
        "cancelling released the hold"
    );
    app.reserve(&buyer, &shop, 2).await; // and gave the quota back
}

#[tokio::test]
async fn organizers_record_cash_payments_for_their_own_events_only() {
    let app = TestApp::start().await;
    let shop = app.shop(10, 1_000, json!({})).await;
    let other_shop = app.shop(10, 1_000, json!({})).await;
    let buyer = app.buyer().await;
    let reservation = app.reserve(&buyer, &shop, 1).await;
    let path = format!(
        "/v1/reservations/{}/manual-payment",
        reservation["id"].as_str().unwrap()
    );
    let body = json!({ "attestation_id": "receipt-001", "amount": reservation["total"] });

    let stranger = app
        .call(
            Method::POST,
            &path,
            Some(&other_shop.organizer),
            Some(body.clone()),
        )
        .await;
    assert_eq!(stranger.status, StatusCode::FORBIDDEN);
    let paid = app
        .call(Method::POST, &path, Some(&shop.organizer), Some(body))
        .await;
    assert_eq!(
        paid.body["reservation"]["status"], "issued",
        "{:?}",
        paid.body
    );
}

#[tokio::test]
async fn drafts_are_invisible_to_the_public() {
    let app = TestApp::start().await;
    let shop = app.shop(10, 1_000, json!({})).await;
    let event = app
        .call(
            Method::GET,
            &format!("/v1/events/{}", shop.event_id),
            None,
            None,
        )
        .await;
    let mut draft = event.body;
    draft["status"] = json!("draft");
    app.call(
        Method::PUT,
        &format!("/v1/events/{}", shop.event_id),
        Some(&shop.organizer),
        Some(draft),
    )
    .await;

    assert_eq!(
        app.call(
            Method::GET,
            &format!("/v1/events/{}", shop.event_id),
            None,
            None
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );
    let buyer = app.buyer().await;
    assert_eq!(
        app.purchase(&buyer, &shop, 1, "k").await.status,
        StatusCode::NOT_FOUND
    );
    let own = app
        .call(
            Method::GET,
            &format!("/v1/events/{}", shop.event_id),
            Some(&shop.organizer),
            None,
        )
        .await;
    assert_eq!(own.status, StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn each_sale_is_first_come_first_served_even_when_sales_run_in_parallel() {
    let app = TestApp::start().await;
    let shops = [
        app.shop(1, 1_000, json!({})).await,
        app.shop(1, 1_000, json!({})).await,
    ];
    let buyer = app.buyer().await;
    // Interleaved arrivals: first, first, second, second.
    let mut submitted = Vec::new();
    for round in 0..2 {
        for shop in &shops {
            app.clock.advance(Duration::milliseconds(1));
            let reply = app.purchase(&buyer, shop, 1, &uuid_suffix()).await;
            assert_eq!(reply.status, StatusCode::ACCEPTED, "{:?}", reply.body);
            submitted.push((round, reply.body["id"].as_str().unwrap().to_owned()));
        }
    }
    app.drain("purchases").await;

    for (round, id) in submitted {
        let request = app
            .call(
                Method::GET,
                &format!("/v1/purchase-requests/{id}"),
                Some(&buyer),
                None,
            )
            .await;
        let expected = if round == 0 { "reserved" } else { "rejected" };
        assert_eq!(request.body["status"], expected, "{:?}", request.body);
    }
}
