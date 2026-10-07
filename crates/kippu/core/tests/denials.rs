//! Deny lists through the HTTP API: organizers keep them per event and per organization, gates
//! read the tickets to refuse, and denied accounts cannot buy.
#![allow(
    clippy::unwrap_used,
    reason = "test helpers: a failure is the test failing"
)]

mod support;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use support::{Shop, TestApp, uuid_suffix};

async fn account_id(app: &TestApp, token: &str) -> String {
    let me = app.call(Method::GET, "/v1/me", Some(token), None).await;
    me.body["account"]["id"].as_str().unwrap().to_owned()
}

async fn organization_of(app: &TestApp, shop: &Shop) -> String {
    let me = app
        .call(Method::GET, "/v1/me", Some(&shop.organizer), None)
        .await;
    me.body["organizations"][0].as_str().unwrap().to_owned()
}

/// Buys `quantity` tickets with cash recorded by the organizer; returns the ticket ids.
async fn tickets_for(app: &TestApp, shop: &Shop, buyer: &str, quantity: u32) -> Vec<String> {
    let reservation = app.reserve(buyer, shop, quantity).await;
    let paid = app
        .call(
            Method::POST,
            &format!(
                "/v1/reservations/{}/manual-payment",
                reservation["id"].as_str().unwrap()
            ),
            Some(&shop.organizer),
            Some(json!({ "attestation_id": uuid_suffix(), "amount": reservation["total"] })),
        )
        .await;
    paid.body["ticket_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| id.as_str().unwrap().to_owned())
        .collect()
}

fn subject(kind: &str, id: &str) -> Value {
    json!({ "kind": kind, "id": id })
}

/// Every ticket id the gate list holds, with its reason, following cursors.
async fn denied_tickets(app: &TestApp, shop: &Shop, token: &str) -> Vec<(String, String)> {
    let mut all = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let path = match &cursor {
            None => format!("/v1/events/{}/denied-tickets?limit=2", shop.event_id),
            Some(cursor) => format!(
                "/v1/events/{}/denied-tickets?limit=2&cursor={cursor}",
                shop.event_id
            ),
        };
        let page = app.call(Method::GET, &path, Some(token), None).await;
        assert_eq!(page.status, StatusCode::OK, "{:?}", page.body);
        for item in page.body["items"].as_array().unwrap() {
            all.push((
                item["ticket_id"].as_str().unwrap().to_owned(),
                item["reason"].as_str().unwrap().to_owned(),
            ));
        }
        match page.body["next_cursor"].as_str() {
            Some(next) => cursor = Some(next.to_owned()),
            None => return all,
        }
    }
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "the life of two denials, told in order"
)]
async fn organizers_keep_deny_lists_for_their_events_and_organization() {
    let app = TestApp::start().await;
    let shop = app.shop(10, 1_000, json!({})).await;
    let organization = organization_of(&app, &shop).await;
    let buyer = app.buyer().await;
    let buyer_id = account_id(&app, &buyer).await;
    let tickets = tickets_for(&app, &shop, &buyer, 1).await;

    let event_denials = format!("/v1/events/{}/denials", shop.event_id);
    let organization_denials = format!("/v1/organizations/{organization}/denials");
    let ticket_denial = app
        .call(
            Method::POST,
            &event_denials,
            Some(&shop.organizer),
            Some(json!({ "subject": subject("ticket", &tickets[0]), "note": "Reported stolen" })),
        )
        .await;
    assert_eq!(
        ticket_denial.status,
        StatusCode::CREATED,
        "{:?}",
        ticket_denial.body
    );
    assert_eq!(ticket_denial.body["event_id"], json!(shop.event_id));
    assert_eq!(ticket_denial.body["organization_id"], json!(organization));
    assert_eq!(ticket_denial.body["subject"]["kind"], "ticket");
    assert_eq!(ticket_denial.body["note"], "Reported stolen");

    // Denying the same thing again changes nothing.
    let again = app
        .call(
            Method::POST,
            &event_denials,
            Some(&shop.organizer),
            Some(json!({ "subject": subject("ticket", &tickets[0]), "note": "Other note" })),
        )
        .await;
    assert_eq!(again.status, StatusCode::OK);
    assert_eq!(again.body, ticket_denial.body);

    let account_denial = app
        .call(
            Method::POST,
            &organization_denials,
            Some(&shop.organizer),
            Some(json!({ "subject": subject("account", &buyer_id) })),
        )
        .await;
    assert_eq!(
        account_denial.status,
        StatusCode::CREATED,
        "{:?}",
        account_denial.body
    );
    assert_eq!(account_denial.body["event_id"], Value::Null);
    assert_eq!(account_denial.body["note"], "");

    // Each list holds its own entries.
    let listed = app
        .call(
            Method::GET,
            &organization_denials,
            Some(&shop.organizer),
            None,
        )
        .await;
    assert_eq!(listed.body["items"], json!([account_denial.body]));
    let listed = app
        .call(Method::GET, &event_denials, Some(&shop.organizer), None)
        .await;
    assert_eq!(listed.body["items"], json!([ticket_denial.body]));

    // Notes change with the version read.
    let path = format!(
        "/v1/denials/{}",
        account_denial.body["id"].as_str().unwrap()
    );
    let edited = app
        .call(
            Method::PATCH,
            &path,
            Some(&shop.organizer),
            Some(json!({ "version": 1, "note": "Harassed staff in 2026" })),
        )
        .await;
    assert_eq!(edited.status, StatusCode::OK, "{:?}", edited.body);
    assert_eq!(edited.body["note"], "Harassed staff in 2026");
    assert_eq!(edited.body["version"], 2);
    let stale = app
        .call(
            Method::PATCH,
            &path,
            Some(&shop.organizer),
            Some(json!({ "version": 1, "note": "Stale" })),
        )
        .await;
    assert_eq!(stale.status, StatusCode::PRECONDITION_FAILED);
    let read = app
        .call(Method::GET, &path, Some(&shop.organizer), None)
        .await;
    assert_eq!(read.body, edited.body);

    // Lifted, it is gone.
    let deleted = app
        .call(Method::DELETE, &path, Some(&shop.organizer), None)
        .await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT);
    let gone = app
        .call(Method::GET, &path, Some(&shop.organizer), None)
        .await;
    assert_eq!(gone.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn deny_lists_are_confined_to_their_organization() {
    let app = TestApp::start().await;
    let shop = app.shop(10, 1_000, json!({})).await;
    let other = app.shop(10, 1_000, json!({})).await;
    let buyer = app.buyer().await;
    let tickets = tickets_for(&app, &shop, &buyer, 1).await;
    let other_tickets = tickets_for(&app, &other, &buyer, 1).await;

    // Another organization's organizer cannot add to or read this event's list.
    let event_denials = format!("/v1/events/{}/denials", shop.event_id);
    let intruder = app
        .call(
            Method::POST,
            &event_denials,
            Some(&other.organizer),
            Some(json!({ "subject": subject("ticket", &tickets[0]) })),
        )
        .await;
    assert_eq!(intruder.status, StatusCode::FORBIDDEN);
    let peek = app
        .call(
            Method::GET,
            &format!("/v1/events/{}/denied-tickets", shop.event_id),
            Some(&other.organizer),
            None,
        )
        .await;
    assert_eq!(peek.status, StatusCode::FORBIDDEN);
    let buyer_peek = app
        .call(Method::GET, &event_denials, Some(&buyer), None)
        .await;
    assert_eq!(buyer_peek.status, StatusCode::FORBIDDEN);

    // Only this event's tickets can be denied for it, and only existing accounts.
    let foreign_ticket = app
        .call(
            Method::POST,
            &event_denials,
            Some(&shop.organizer),
            Some(json!({ "subject": subject("ticket", &other_tickets[0]) })),
        )
        .await;
    assert_eq!(foreign_ticket.status, StatusCode::UNPROCESSABLE_ENTITY);
    let nobody = app
        .call(
            Method::POST,
            &event_denials,
            Some(&shop.organizer),
            Some(json!({ "subject": subject("account", &uuid::Uuid::now_v7().to_string()) })),
        )
        .await;
    assert_eq!(nobody.status, StatusCode::UNPROCESSABLE_ENTITY);
    let unknown_field = app
        .call(
            Method::POST,
            &event_denials,
            Some(&shop.organizer),
            Some(json!({ "subject": subject("ticket", &tickets[0]), "reason": "typo" })),
        )
        .await;
    assert_eq!(unknown_field.status, StatusCode::UNPROCESSABLE_ENTITY);

    // A denial of this organization is invisible to the other's organizer.
    let denied = app
        .call(
            Method::POST,
            &event_denials,
            Some(&shop.organizer),
            Some(json!({ "subject": subject("ticket", &tickets[0]) })),
        )
        .await;
    let path = format!("/v1/denials/{}", denied.body["id"].as_str().unwrap());
    for method in [Method::GET, Method::DELETE] {
        let hidden = app.call(method, &path, Some(&other.organizer), None).await;
        assert_eq!(hidden.status, StatusCode::NOT_FOUND);
    }
}

#[tokio::test]
async fn gates_learn_every_ticket_to_refuse() {
    let app = TestApp::start().await;
    let shop = app.shop(20, 1_000, json!({})).await;
    let organization = organization_of(&app, &shop).await;

    let stolen_from = app.buyer().await;
    let stolen = tickets_for(&app, &shop, &stolen_from, 1).await;
    let banned = app.buyer().await;
    let banned_tickets = tickets_for(&app, &shop, &banned, 2).await;
    let changed_mind = app.buyer().await;
    let refunded = tickets_for(&app, &shop, &changed_mind, 1).await;
    let welcome = app.buyer().await;
    let admitted = tickets_for(&app, &shop, &welcome, 2).await;

    app.call(
        Method::POST,
        &format!("/v1/events/{}/denials", shop.event_id),
        Some(&shop.organizer),
        Some(json!({ "subject": subject("ticket", &stolen[0]) })),
    )
    .await;
    let ban = app
        .call(
            Method::POST,
            &format!("/v1/organizations/{organization}/denials"),
            Some(&shop.organizer),
            Some(json!({ "subject": subject("account", &account_id(&app, &banned).await) })),
        )
        .await;
    let reservations = app
        .call(
            Method::GET,
            "/v1/me/reservations",
            Some(&changed_mind),
            None,
        )
        .await;
    let refund = app
        .call(
            Method::POST,
            &format!(
                "/v1/reservations/{}/refunds",
                reservations.body["items"][0]["id"].as_str().unwrap()
            ),
            Some(&shop.organizer),
            Some(json!({})),
        )
        .await;
    assert_eq!(refund.status, StatusCode::CREATED, "{:?}", refund.body);

    let mut expected = vec![
        (stolen[0].clone(), "denied".to_owned()),
        (banned_tickets[0].clone(), "denied".to_owned()),
        (banned_tickets[1].clone(), "denied".to_owned()),
        (refunded[0].clone(), "revoked".to_owned()),
    ];
    expected.sort();
    let listed = denied_tickets(&app, &shop, &shop.organizer).await;
    assert_eq!(listed, expected);
    assert!(listed.iter().all(|(ticket, _)| !admitted.contains(ticket)));

    // A banned account cannot buy more: its purchase requests are rejected.
    let attempt = app.purchase(&banned, &shop, 1, &uuid_suffix()).await;
    assert_eq!(attempt.status, StatusCode::ACCEPTED, "{:?}", attempt.body);
    app.drain("purchases").await;
    let request = app
        .call(
            Method::GET,
            &format!(
                "/v1/purchase-requests/{}",
                attempt.body["id"].as_str().unwrap()
            ),
            Some(&banned),
            None,
        )
        .await;
    assert_eq!(request.body["status"], "rejected");
    assert_eq!(request.body["reason"], "account_denied");

    // Lifting the ban lets the account's tickets in again.
    app.call(
        Method::DELETE,
        &format!("/v1/denials/{}", ban.body["id"].as_str().unwrap()),
        Some(&shop.organizer),
        None,
    )
    .await;
    let listed = denied_tickets(&app, &shop, &shop.organizer).await;
    assert_eq!(listed.len(), 2);
    assert!(
        listed
            .iter()
            .all(|(ticket, _)| !banned_tickets.contains(ticket))
    );
}
