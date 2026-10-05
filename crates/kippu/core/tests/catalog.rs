//! Editing the catalog: partial updates with `PATCH` and optimistic concurrency.

mod support;

use axum::http::{Method, StatusCode};
use serde_json::json;
use support::TestApp;

#[tokio::test]
async fn patch_changes_only_the_given_fields() {
    let app = TestApp::start().await;
    let shop = app.shop(10, 1_000, json!({})).await;
    let path = format!("/v1/events/{}", shop.event_id);
    let before = app.call(Method::GET, &path, None, None).await.body;

    let patched = app
        .call(
            Method::PATCH,
            &path,
            Some(&shop.organizer),
            Some(json!({ "version": before["version"], "title": "Comic Market 111" })),
        )
        .await;
    assert_eq!(patched.status, StatusCode::OK, "{:?}", patched.body);
    assert_eq!(patched.body["title"], "Comic Market 111");
    assert_eq!(patched.body["venue"], before["venue"]);
    assert_eq!(patched.body["slug"], before["slug"]);
    assert_eq!(
        patched.body["version"],
        before["version"].as_i64().unwrap() + 1
    );
    let stored = app.call(Method::GET, &path, None, None).await.body;
    assert_eq!(stored, patched.body);

    // The same patch again carries a version that is no longer stored.
    let stale = app
        .call(
            Method::PATCH,
            &path,
            Some(&shop.organizer),
            Some(json!({ "version": before["version"], "venue": "Makuhari Messe" })),
        )
        .await;
    assert_eq!(stale.status, StatusCode::PRECONDITION_FAILED);
    assert_eq!(stale.body["type"], "urn:kippu:problem:stale-version");
    assert_eq!(
        app.call(Method::GET, &path, None, None).await.body["venue"],
        before["venue"]
    );
}

#[tokio::test]
async fn patch_validates_the_merged_record() {
    let app = TestApp::start().await;
    let shop = app.shop(10, 1_000, json!({})).await;
    let path = format!("/v1/events/{}", shop.event_id);
    let event = app.call(Method::GET, &path, None, None).await.body;

    let backwards = app
        .call(
            Method::PATCH,
            &path,
            Some(&shop.organizer),
            Some(json!({ "version": event["version"], "ends_at": event["starts_at"] })),
        )
        .await;
    assert_eq!(backwards.status, StatusCode::UNPROCESSABLE_ENTITY);

    let typo = app
        .call(
            Method::PATCH,
            &path,
            Some(&shop.organizer),
            Some(json!({ "version": event["version"], "titel": "Oops" })),
        )
        .await;
    assert_eq!(typo.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(typo.headers["content-type"], "application/problem+json");
    assert_eq!(typo.body["type"], "urn:kippu:problem:invalid-json");
    assert!(
        typo.body["detail"].as_str().unwrap().contains("titel"),
        "{:?}",
        typo.body
    );

    let stranger = app.buyer().await;
    let forbidden = app
        .call(
            Method::PATCH,
            &path,
            Some(&stranger),
            Some(json!({ "version": event["version"], "title": "Mine" })),
        )
        .await;
    assert_eq!(forbidden.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn sales_and_ticket_types_can_be_patched() {
    let app = TestApp::start().await;
    let shop = app.shop(10, 1_000, json!({})).await;

    let sale_path = format!("/v1/sales/{}", shop.sale_id);
    let sale = app.call(Method::GET, &sale_path, None, None).await.body;
    let patched = app
        .call_with(
            Method::PATCH,
            &sale_path,
            Some(&shop.organizer),
            Some(json!({ "version": sale["version"], "max_tickets_per_request": 2 })),
            &[("content-type", "application/merge-patch+json")],
        )
        .await;
    assert_eq!(patched.status, StatusCode::OK, "{:?}", patched.body);
    assert_eq!(patched.body["max_tickets_per_request"], 2);
    assert_eq!(patched.body["name"], sale["name"]);
    assert_eq!(patched.body["admission"], sale["admission"]);

    let ticket_type_path = format!("/v1/ticket-types/{}", shop.ticket_type_id);
    let ticket_type = &sale["ticket_types"][0];
    let raised = app
        .call(
            Method::PATCH,
            &ticket_type_path,
            Some(&shop.organizer),
            Some(json!({ "version": ticket_type["version"], "capacity": 20 })),
        )
        .await;
    assert_eq!(raised.status, StatusCode::OK, "{:?}", raised.body);
    assert_eq!(raised.body["capacity"], 20);
    assert_eq!(raised.body["price"], ticket_type["price"]);

    let after = app.call(Method::GET, &sale_path, None, None).await.body;
    assert_eq!(after["ticket_types"][0]["available"], 20);
}

#[tokio::test]
async fn patch_is_documented_as_merge_patch() {
    let app = TestApp::start().await;
    let openapi = app
        .call(Method::GET, "/openapi.json", None, None)
        .await
        .body;
    for path in [
        "/v1/events/{event_id}",
        "/v1/sales/{sale_id}",
        "/v1/ticket-types/{ticket_type_id}",
    ] {
        let content = &openapi["paths"][path]["patch"]["requestBody"]["content"];
        assert!(
            content["application/merge-patch+json"].is_object(),
            "{path}"
        );
        assert!(content["application/json"].is_object(), "{path}");
    }
}
