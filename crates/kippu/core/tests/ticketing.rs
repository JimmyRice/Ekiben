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
use support::TestApp;

/// A buyer with one free ticket, and that ticket.
async fn ticket_holder(app: &TestApp) -> (String, Value) {
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
    (buyer, tickets.body["items"][0].clone())
}

#[tokio::test]
async fn the_raw_endpoint_returns_the_bytes_the_json_encodes() {
    let app = TestApp::start().await;
    let (buyer, ticket) = ticket_holder(&app).await;
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
    let (_, ticket) = ticket_holder(&app).await;
    let path = format!("/v1/tickets/{}/raw", ticket["id"].as_str().unwrap());

    let stranger = app.buyer().await;
    let hidden = app.call(Method::GET, &path, Some(&stranger), None).await;
    assert_eq!(hidden.status, StatusCode::NOT_FOUND);
    assert_eq!(hidden.body["type"], "urn:kippu:problem:not-found");
    let anonymous = app.call(Method::GET, &path, None, None).await;
    assert_eq!(anonymous.status, StatusCode::UNAUTHORIZED);
}
