//! Refunds of issued tickets through the HTTP API: who may refund when, what happens to the
//! tickets and the stock, and how attestors confirm or reverse.
#![allow(
    clippy::unwrap_used,
    reason = "test helpers: a failure is the test failing"
)]

mod support;

use axum::http::{Method, StatusCode};
use kippu_domain::Duration;
use serde_json::{Value, json};
use support::{Attestor, Shop, TestApp, uuid_suffix};

/// Lets buyers refund the shop's tickets themselves until `until` (or not, with `null`).
async fn refundable_until(app: &TestApp, shop: &Shop, until: Value) -> TicketTypeReply {
    let path = format!("/v1/ticket-types/{}", shop.ticket_type_id);
    let current = app.call(Method::GET, &path, None, None).await;
    let patched = app
        .call(
            Method::PATCH,
            &path,
            Some(&shop.organizer),
            Some(json!({ "version": current.body["version"], "refundable_until": until })),
        )
        .await;
    TicketTypeReply {
        status: patched.status,
        body: patched.body,
    }
}

struct TicketTypeReply {
    status: StatusCode,
    body: Value,
}

/// Buys `quantity` tickets and pays for them through `attestor`. Returns the reservation and
/// the ticket ids.
async fn bought(
    app: &TestApp,
    shop: &Shop,
    attestor: &Attestor,
    buyer: &str,
    quantity: u32,
) -> (Value, Vec<String>) {
    let reservation = app.reserve(buyer, shop, quantity).await;
    let id = reservation["id"].as_str().unwrap();
    let checkout = app
        .call(
            Method::POST,
            &format!("/v1/reservations/{id}/checkout"),
            Some(buyer),
            Some(json!({ "attestor_id": attestor.id })),
        )
        .await;
    assert_eq!(checkout.status, StatusCode::OK, "{:?}", checkout.body);
    let paid = app
        .pay(attestor, &format!("pi_{}", uuid_suffix()), &reservation)
        .await;
    assert_eq!(paid.body["disposition"], "applied", "{:?}", paid.body);
    let tickets = paid.body["ticket_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| id.as_str().unwrap().to_owned())
        .collect();
    (paid.body["reservation"].clone(), tickets)
}

/// A shop whose sale accepts a fresh live attestor.
async fn shop_with_attestor(app: &TestApp, capacity: u32, price_minor: i64) -> (Shop, Attestor) {
    let shop = app.shop(capacity, price_minor, json!({})).await;
    let attestor = app.attestor("live").await;
    app.accept_attestor(&shop, &attestor.id).await;
    (shop, attestor)
}

async fn refund(app: &TestApp, token: &str, reservation: &Value, body: Value) -> support::Reply {
    app.call(
        Method::POST,
        &format!(
            "/v1/reservations/{}/refunds",
            reservation["id"].as_str().unwrap()
        ),
        Some(token),
        Some(body),
    )
    .await
}

async fn available(app: &TestApp, shop: &Shop) -> u64 {
    app.call(
        Method::GET,
        &format!("/v1/ticket-types/{}", shop.ticket_type_id),
        None,
        None,
    )
    .await
    .body["available"]
        .as_u64()
        .unwrap()
}

async fn ticket_status(app: &TestApp, buyer: &str, ticket: &str) -> Value {
    app.call(
        Method::GET,
        &format!("/v1/tickets/{ticket}"),
        Some(buyer),
        None,
    )
    .await
    .body["status"]
        .clone()
}

/// The events of the admin feed with `topic`.
async fn feed_events(app: &TestApp, topic: &str) -> Vec<Value> {
    let feed = app
        .call(
            Method::GET,
            "/v1/admin/feed?limit=500",
            Some(&app.root_token()),
            None,
        )
        .await;
    feed.body
        .as_array()
        .unwrap()
        .iter()
        .map(|record| record["event"].clone())
        .filter(|event| event["topic"] == topic)
        .collect()
}

#[tokio::test]
#[expect(clippy::too_many_lines, reason = "one refund's journey, told in order")]
async fn a_buyer_refunds_within_the_refund_period_and_the_attestor_confirms() {
    let app = TestApp::start().await;
    let (shop, attestor) = shop_with_attestor(&app, 10, 3_000).await;
    let until = (app.now() + Duration::days(7)).to_string();
    let patched = refundable_until(&app, &shop, json!(until)).await;
    assert_eq!(patched.status, StatusCode::OK, "{:?}", patched.body);
    assert_eq!(patched.body["refundable_until"], json!(until));
    let buyer = app.buyer().await;
    let (reservation, tickets) = bought(&app, &shop, &attestor, &buyer, 2).await;
    assert_eq!(available(&app, &shop).await, 8);

    let refunded = refund(
        &app,
        &buyer,
        &reservation,
        json!({ "ticket_ids": [tickets[0]] }),
    )
    .await;
    assert_eq!(refunded.status, StatusCode::CREATED, "{:?}", refunded.body);
    assert_eq!(refunded.body["status"], "pending");
    assert_eq!(refunded.body["reason"], "requested");
    assert_eq!(refunded.body["ticket_ids"], json!([tickets[0]]));
    assert_eq!(
        refunded.body["amount"],
        json!({ "amount_minor": 3000, "currency": "JPY" })
    );
    let refund_id = refunded.body["id"].as_str().unwrap().to_owned();

    // The ticket is dead at once, its seat is back on sale, the other ticket still works.
    assert_eq!(ticket_status(&app, &buyer, &tickets[0]).await, "revoked");
    assert_eq!(ticket_status(&app, &buyer, &tickets[1]).await, "valid");
    assert_eq!(available(&app, &shop).await, 9);

    // The attestor is asked to return exactly this refund's money.
    let feed = app
        .signed(&attestor, Method::GET, "/v1/attestor/feed", None)
        .await;
    let owed: Vec<&Value> = feed
        .body
        .as_array()
        .unwrap()
        .iter()
        .map(|record| &record["event"])
        .filter(|event| event["topic"] == "refund.required")
        .collect();
    assert_eq!(owed.len(), 1, "{:?}", feed.body);
    assert_eq!(owed[0]["refund_id"], json!(refund_id));
    assert_eq!(owed[0]["amount"], refunded.body["amount"]);
    let attestation_id = owed[0]["attestation_id"].as_str().unwrap().to_owned();
    let revoked = feed_events(&app, "tickets.revoked").await;
    assert_eq!(revoked.len(), 1);
    assert_eq!(revoked[0]["ticket_ids"], json!([tickets[0]]));
    assert_eq!(revoked[0]["event_id"], json!(shop.event_id));
    assert_eq!(revoked[0]["reason"], "requested");

    let confirmation = json!({
        "attestation_id": attestation_id,
        "outcome": "refunded",
        "refund_id": refund_id,
    });
    let confirmed = app
        .signed(
            &attestor,
            Method::POST,
            "/v1/payment-attestations",
            Some(confirmation.clone()),
        )
        .await;
    assert_eq!(confirmed.status, StatusCode::OK, "{:?}", confirmed.body);
    assert_eq!(confirmed.body["disposition"], "applied");
    assert_eq!(confirmed.body["replayed"], false);
    assert_eq!(confirmed.body["refund"]["status"], "completed");
    let again = app
        .signed(
            &attestor,
            Method::POST,
            "/v1/payment-attestations",
            Some(confirmation),
        )
        .await;
    assert_eq!(again.body["replayed"], true, "{:?}", again.body);
    assert_eq!(again.body["refund"], confirmed.body["refund"]);

    // Without a refund_id, a payment for issued tickets cannot be marked refunded.
    let bare = app
        .signed(
            &attestor,
            Method::POST,
            "/v1/payment-attestations",
            Some(json!({ "attestation_id": attestation_id, "outcome": "refunded" })),
        )
        .await;
    assert_eq!(bare.status, StatusCode::CONFLICT);
    assert_eq!(bare.body["type"], "urn:kippu:problem:payment-applied");

    // The rest of the reservation: `{}` refunds every ticket still valid, then nothing is left.
    let rest = refund(&app, &buyer, &reservation, json!({})).await;
    assert_eq!(rest.status, StatusCode::CREATED, "{:?}", rest.body);
    assert_eq!(rest.body["ticket_ids"], json!([tickets[1]]));
    let none_left = refund(&app, &buyer, &reservation, json!({})).await;
    assert_eq!(none_left.status, StatusCode::CONFLICT);
    assert_eq!(
        none_left.body["type"],
        "urn:kippu:problem:nothing-to-refund"
    );
    let twice = refund(
        &app,
        &buyer,
        &reservation,
        json!({ "ticket_ids": [tickets[0]] }),
    )
    .await;
    assert_eq!(twice.body["type"], "urn:kippu:problem:ticket-revoked");
    assert_eq!(available(&app, &shop).await, 10);

    let listed = app
        .call(
            Method::GET,
            &format!(
                "/v1/reservations/{}/refunds",
                reservation["id"].as_str().unwrap()
            ),
            Some(&buyer),
            None,
        )
        .await;
    let listed = listed.body["items"].as_array().unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0]["id"], json!(refund_id));
    assert_eq!(listed[0]["status"], "completed");
    assert_eq!(listed[1]["status"], "pending");
    let one = app
        .call(
            Method::GET,
            &format!("/v1/refunds/{refund_id}"),
            Some(&shop.organizer),
            None,
        )
        .await;
    assert_eq!(one.body["status"], "completed");
}

#[tokio::test]
async fn buyers_refund_only_refundable_tickets_in_time_but_organizers_always_can() {
    let app = TestApp::start().await;
    let (shop, attestor) = shop_with_attestor(&app, 10, 1_000).await;
    let buyer = app.buyer().await;
    let (reservation, _) = bought(&app, &shop, &attestor, &buyer, 1).await;

    // Not refundable by buyers.
    let refused = refund(&app, &buyer, &reservation, json!({})).await;
    assert_eq!(refused.status, StatusCode::CONFLICT);
    assert_eq!(refused.body["type"], "urn:kippu:problem:refund-not-allowed");

    // Refundable for ten minutes; eleven minutes later, too late.
    let until = (app.now() + Duration::minutes(10)).to_string();
    refundable_until(&app, &shop, json!(until)).await;
    app.clock.advance(Duration::minutes(11));
    let late = refund(&app, &buyer, &reservation, json!({})).await;
    assert_eq!(late.body["type"], "urn:kippu:problem:refund-not-allowed");

    // Strangers do not see the reservation; another event's organizer neither.
    let stranger = app.buyer().await;
    let hidden = refund(&app, &stranger, &reservation, json!({})).await;
    assert_eq!(hidden.status, StatusCode::NOT_FOUND);
    let other = app.shop(10, 1_000, json!({})).await;
    let hidden = refund(&app, &other.organizer, &reservation, json!({})).await;
    assert_eq!(hidden.status, StatusCode::NOT_FOUND);

    // The event's organizer refunds whenever needed.
    let refunded = refund(&app, &shop.organizer, &reservation, json!({})).await;
    assert_eq!(refunded.status, StatusCode::CREATED, "{:?}", refunded.body);
    assert_eq!(refunded.body["status"], "pending");

    // Tickets that are not the reservation's are refused.
    let unknown = refund(
        &app,
        &shop.organizer,
        &reservation,
        json!({ "ticket_ids": [uuid::Uuid::now_v7()] }),
    )
    .await;
    assert_eq!(unknown.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        unknown.body["type"],
        "urn:kippu:problem:ticket-not-in-reservation"
    );

    // An unpaid reservation has nothing to refund.
    let unpaid = app.reserve(&buyer, &shop, 1).await;
    let not_issued = refund(&app, &shop.organizer, &unpaid, json!({})).await;
    assert_eq!(not_issued.status, StatusCode::CONFLICT);
    assert_eq!(
        not_issued.body["type"],
        "urn:kippu:problem:reservation-not-issued"
    );
}

#[tokio::test]
async fn the_refund_period_ends_before_the_tickets_are_valid_and_the_event_starts() {
    let app = TestApp::start().await;
    let shop = app.shop(10, 1_000, json!({})).await;
    // The shop's event starts, and its tickets are valid, in 30 days.
    let too_late = (app.now() + Duration::days(31)).to_string();
    let refused = refundable_until(&app, &shop, json!(too_late)).await;
    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        refused.body["detail"]
            .as_str()
            .unwrap()
            .contains("refundable_until"),
        "{:?}",
        refused.body
    );
    let fine = (app.now() + Duration::days(29)).to_string();
    assert_eq!(
        refundable_until(&app, &shop, json!(fine)).await.status,
        StatusCode::OK
    );
    let removed = refundable_until(&app, &shop, Value::Null).await;
    assert_eq!(removed.status, StatusCode::OK);
    assert_eq!(removed.body["refundable_until"], Value::Null);
}

#[tokio::test]
async fn a_reversal_revokes_the_tickets_it_paid_for() {
    let app = TestApp::start().await;
    let (shop, attestor) = shop_with_attestor(&app, 10, 2_000).await;
    let buyer = app.buyer().await;
    let (reservation, tickets) = bought(&app, &shop, &attestor, &buyer, 2).await;
    let refunded = refund(
        &app,
        &shop.organizer,
        &reservation,
        json!({ "ticket_ids": [tickets[0]] }),
    )
    .await;
    let attestation_id = refunded.body["attestation_id"].as_str().unwrap().to_owned();

    // A chargeback: the attestor reports it, the remaining ticket is revoked.
    let report = json!({ "attestation_id": attestation_id, "outcome": "reversed" });
    let reversed = app
        .signed(
            &attestor,
            Method::POST,
            "/v1/payment-attestations",
            Some(report.clone()),
        )
        .await;
    assert_eq!(reversed.status, StatusCode::OK, "{:?}", reversed.body);
    assert_eq!(reversed.body["replayed"], false);
    assert_eq!(reversed.body["refund"]["reason"], "reversal");
    assert_eq!(reversed.body["refund"]["status"], "completed");
    assert_eq!(reversed.body["refund"]["ticket_ids"], json!([tickets[1]]));
    assert_eq!(
        reversed.body["refund"]["amount"],
        json!({ "amount_minor": 2000, "currency": "JPY" })
    );
    assert_eq!(ticket_status(&app, &buyer, &tickets[1]).await, "revoked");
    assert_eq!(available(&app, &shop).await, 10);
    // The refund still pending went back with the payment: nothing more to pay out.
    let earlier = app
        .call(
            Method::GET,
            &format!("/v1/refunds/{}", refunded.body["id"].as_str().unwrap()),
            Some(&shop.organizer),
            None,
        )
        .await;
    assert_eq!(earlier.body["status"], "completed", "{:?}", earlier.body);

    for _ in 0..3 {
        let again = app
            .signed(
                &attestor,
                Method::POST,
                "/v1/payment-attestations",
                Some(report.clone()),
            )
            .await;
        assert_eq!(again.body["replayed"], true, "{:?}", again.body);
        assert_eq!(again.body["refund"], reversed.body["refund"]);
    }
    assert_eq!(available(&app, &shop).await, 10);
    let revoked = feed_events(&app, "tickets.revoked").await;
    assert_eq!(revoked.len(), 2);
    assert_eq!(revoked[1]["reason"], "reversal");

    // A refund_id only goes with `refunded`.
    let mixed = app
        .signed(
            &attestor,
            Method::POST,
            "/v1/payment-attestations",
            Some(json!({
                "attestation_id": attestation_id,
                "outcome": "reversed",
                "refund_id": refunded.body["id"],
            })),
        )
        .await;
    assert_eq!(mixed.status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn free_tickets_are_refunded_at_once() {
    let app = TestApp::start().await;
    let shop = app.shop(10, 0, json!({})).await;
    let until = (app.now() + Duration::days(7)).to_string();
    refundable_until(&app, &shop, json!(until)).await;
    let buyer = app.buyer().await;
    let reservation = app.reserve(&buyer, &shop, 1).await;
    app.call(
        Method::POST,
        &format!(
            "/v1/reservations/{}/checkout",
            reservation["id"].as_str().unwrap()
        ),
        Some(&buyer),
        Some(json!({})),
    )
    .await;

    let refunded = refund(&app, &buyer, &reservation, json!({})).await;
    assert_eq!(refunded.status, StatusCode::CREATED, "{:?}", refunded.body);
    assert_eq!(refunded.body["status"], "completed");
    assert_eq!(refunded.body["amount"]["amount_minor"], 0);
    assert_eq!(
        feed_events(&app, "refund.required").await,
        Vec::<Value>::new()
    );
    assert_eq!(available(&app, &shop).await, 10);
}

#[tokio::test]
async fn organizers_confirm_cash_refunds() {
    let app = TestApp::start().await;
    let shop = app.shop(10, 1_000, json!({})).await;
    let buyer = app.buyer().await;
    let reservation = app.reserve(&buyer, &shop, 1).await;
    let paid = app
        .call(
            Method::POST,
            &format!(
                "/v1/reservations/{}/manual-payment",
                reservation["id"].as_str().unwrap()
            ),
            Some(&shop.organizer),
            Some(json!({ "attestation_id": "receipt-7", "amount": reservation["total"] })),
        )
        .await;
    assert_eq!(paid.body["disposition"], "applied", "{:?}", paid.body);

    let refunded = refund(&app, &shop.organizer, &reservation, json!({})).await;
    assert_eq!(refunded.body["status"], "pending", "{:?}", refunded.body);
    let path = format!(
        "/v1/refunds/{}/manual-confirmation",
        refunded.body["id"].as_str().unwrap()
    );
    let by_buyer = app.call(Method::POST, &path, Some(&buyer), None).await;
    assert_eq!(by_buyer.status, StatusCode::FORBIDDEN);
    let confirmed = app
        .call(Method::POST, &path, Some(&shop.organizer), None)
        .await;
    assert_eq!(confirmed.status, StatusCode::OK, "{:?}", confirmed.body);
    assert_eq!(confirmed.body["status"], "completed");
    let again = app
        .call(Method::POST, &path, Some(&shop.organizer), None)
        .await;
    assert_eq!(again.body, confirmed.body);
}

#[tokio::test]
async fn only_manual_refunds_are_confirmed_by_organizers() {
    let app = TestApp::start().await;
    let (shop, attestor) = shop_with_attestor(&app, 10, 1_000).await;
    let buyer = app.buyer().await;
    let (reservation, _) = bought(&app, &shop, &attestor, &buyer, 1).await;
    let refunded = refund(&app, &shop.organizer, &reservation, json!({})).await;
    let confirmed = app
        .call(
            Method::POST,
            &format!(
                "/v1/refunds/{}/manual-confirmation",
                refunded.body["id"].as_str().unwrap()
            ),
            Some(&shop.organizer),
            None,
        )
        .await;
    assert_eq!(confirmed.status, StatusCode::CONFLICT);
    assert_eq!(
        confirmed.body["type"],
        "urn:kippu:problem:refund-not-manual"
    );
}
