//! Delivering tickets: Base64 in JSON, or the raw KP1 bytes.
#![allow(
    clippy::unwrap_used,
    reason = "test helpers: a failure is the test failing"
)]

mod support;

use axum::http::{Method, StatusCode};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};
use support::{Shop, TestApp};

/// A buyer with one free ticket, that ticket, and where it was bought.
async fn ticket_holder(app: &TestApp) -> (String, Value, Shop) {
    let shop = app.shop(10, 0, json!({})).await;
    let buyer = app.buyer().await;
    let reservation = app.reserve(&buyer, &shop, 1).await;
    let path = format!(
        "/v1/reservations/{}/checkout",
        reservation["id"].as_str().unwrap()
    );
    let checkout = app
        .call(Method::POST, &path, Some(&buyer), Some(json!({})))
        .await;
    assert_eq!(checkout.body["status"], "issued", "{:?}", checkout.body);
    let tickets = app
        .call(Method::GET, "/v1/me/tickets", Some(&buyer), None)
        .await;
    (buyer, tickets.body["items"][0].clone(), shop)
}

#[tokio::test]
async fn the_raw_endpoint_returns_the_bytes_the_json_encodes() {
    let app = TestApp::start().await;
    let (buyer, ticket, _) = ticket_holder(&app).await;
    let encoded = ticket["ticket"].as_str().unwrap();
    assert!(
        encoded.starts_with("S1AB"),
        "Base64 of the KP1 header: {encoded}"
    );

    let raw = app
        .call(
            Method::GET,
            &format!("/v1/tickets/{}/raw", ticket["id"].as_str().unwrap()),
            Some(&buyer),
            None,
        )
        .await;
    assert_eq!(raw.status, StatusCode::OK);
    assert_eq!(raw.headers["content-type"], "application/octet-stream");
    assert_eq!(raw.bytes, STANDARD.decode(encoded).unwrap());
    assert!(raw.bytes.starts_with(b"KP"));
}

#[tokio::test]
async fn other_peoples_raw_tickets_are_not_found() {
    let app = TestApp::start().await;
    let (_, ticket, _) = ticket_holder(&app).await;
    let path = format!("/v1/tickets/{}/raw", ticket["id"].as_str().unwrap());

    let stranger = app.buyer().await;
    let hidden = app.call(Method::GET, &path, Some(&stranger), None).await;
    assert_eq!(hidden.status, StatusCode::NOT_FOUND);
    assert_eq!(hidden.body["type"], "urn:kippu:problem:not-found");
    let anonymous = app.call(Method::GET, &path, None, None).await;
    assert_eq!(anonymous.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn tickets_carry_the_current_name_of_their_type() {
    let app = TestApp::start().await;
    let (buyer, ticket, shop) = ticket_holder(&app).await;
    assert_eq!(ticket["ticket_type_id"], shop.ticket_type_id.as_str());
    assert_eq!(ticket["ticket_type_name"], "Day 1");

    let path = format!("/v1/ticket-types/{}", shop.ticket_type_id);
    let ticket_type = app.call(Method::GET, &path, None, None).await.body;
    let renamed = app
        .call(
            Method::PATCH,
            &path,
            Some(&shop.organizer),
            Some(json!({ "version": ticket_type["version"], "name": "Day 1 (East)" })),
        )
        .await;
    assert_eq!(renamed.status, StatusCode::OK, "{:?}", renamed.body);

    let fetched = app
        .call(
            Method::GET,
            &format!("/v1/tickets/{}", ticket["id"].as_str().unwrap()),
            Some(&buyer),
            None,
        )
        .await;
    assert_eq!(fetched.status, StatusCode::OK, "{:?}", fetched.body);
    assert_eq!(fetched.body["ticket_type_name"], "Day 1 (East)");
    assert_eq!(
        fetched.body["ticket"], ticket["ticket"],
        "the signed ticket does not change"
    );
}

#[tokio::test]
async fn organizers_see_their_events_tickets_without_the_signed_bytes() {
    let app = TestApp::start().await;
    let (buyer, ticket, shop) = ticket_holder(&app).await;
    let other_shop = app.shop(10, 0, json!({})).await;
    let ticket_id = ticket["id"].as_str().unwrap();
    let reservation_id = ticket["reservation_id"].as_str().unwrap();
    let path = format!("/v1/tickets/{ticket_id}");
    let of_reservation = format!("/v1/reservations/{reservation_id}/tickets");

    let held = app.call(Method::GET, &path, Some(&buyer), None).await;
    assert!(held.body["ticket"].is_string(), "{:?}", held.body);
    let listed = app
        .call(Method::GET, &of_reservation, Some(&buyer), None)
        .await;
    assert_eq!(listed.status, StatusCode::OK, "{:?}", listed.body);
    assert_eq!(listed.body["items"], json!([held.body]));

    // The organizer sees everything but the bytes, which would let them in.
    let seen = app
        .call(Method::GET, &path, Some(&shop.organizer), None)
        .await;
    assert_eq!(seen.status, StatusCode::OK, "{:?}", seen.body);
    let mut without_bytes = held.body.clone();
    without_bytes["ticket"] = Value::Null;
    assert_eq!(seen.body, without_bytes);
    let listed = app
        .call(Method::GET, &of_reservation, Some(&shop.organizer), None)
        .await;
    assert_eq!(listed.body["items"], json!([without_bytes]));
    let raw = app
        .call(
            Method::GET,
            &format!("{path}/raw"),
            Some(&shop.organizer),
            None,
        )
        .await;
    assert_eq!(raw.status, StatusCode::NOT_FOUND);

    for path in [&path, &of_reservation] {
        let stranger = app
            .call(Method::GET, path, Some(&other_shop.organizer), None)
            .await;
        assert_eq!(stranger.status, StatusCode::NOT_FOUND, "{path}");
    }
}

#[tokio::test]
async fn a_buyers_tickets_are_listed_per_event() {
    let app = TestApp::start().await;
    let buyer = app.buyer().await;
    let mut events = Vec::new();
    for _ in 0..2 {
        let shop = app.shop(10, 0, json!({})).await;
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
        events.push(shop.event_id);
    }

    let all = app
        .call(Method::GET, "/v1/me/tickets", Some(&buyer), None)
        .await;
    assert_eq!(all.body["items"].as_array().unwrap().len(), 2);
    for event in &events {
        let one = app
            .call(
                Method::GET,
                &format!("/v1/me/tickets?event_id={event}"),
                Some(&buyer),
                None,
            )
            .await;
        assert_eq!(one.status, StatusCode::OK, "{:?}", one.body);
        let listed: Vec<&Value> = one.body["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|ticket| &ticket["event_id"])
            .collect();
        assert_eq!(listed, [&json!(event)]);
    }
    let malformed = app
        .call(
            Method::GET,
            "/v1/me/tickets?event_id=not-an-id",
            Some(&buyer),
            None,
        )
        .await;
    assert_eq!(malformed.status, StatusCode::BAD_REQUEST);
}
