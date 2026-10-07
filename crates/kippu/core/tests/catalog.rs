//! Editing the catalog: partial updates with `PATCH`, optimistic concurrency and event content.

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

#[tokio::test]
async fn event_content_is_stored_as_is_and_left_out_of_listings() {
    let app = TestApp::start().await;
    let shop = app.shop(10, 1_000, json!({})).await;
    let path = format!("/v1/events/{}", shop.event_id);
    let event = app.call(Method::GET, &path, None, None).await.body;
    assert_eq!(event["content"], "", "content defaults to empty");

    let page = r#"{"blocks": [{"type": "hero", "text": "<b>駅弁</b>"}]}"#;
    let patched = app
        .call(
            Method::PATCH,
            &path,
            Some(&shop.organizer),
            Some(json!({ "version": event["version"], "content": page })),
        )
        .await;
    assert_eq!(patched.status, StatusCode::OK, "{:?}", patched.body);
    assert_eq!(
        app.call(Method::GET, &path, None, None).await.body["content"],
        page
    );

    let listed = app.call(Method::GET, "/v1/events", None, None).await.body;
    let summary = listed["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|summary| summary["id"] == event["id"])
        .unwrap();
    assert_eq!(summary["title"], event["title"]);
    assert!(summary.get("content").is_none(), "{summary:?}");

    let too_long = app
        .call(
            Method::PATCH,
            &path,
            Some(&shop.organizer),
            Some(json!({
                "version": patched.body["version"],
                "content": "a".repeat(256 * 1024 + 1),
            })),
        )
        .await;
    assert_eq!(too_long.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        too_long.body["detail"]
            .as_str()
            .unwrap()
            .contains("content"),
        "{:?}",
        too_long.body
    );
}

#[tokio::test]
async fn events_carry_a_structured_address_besides_the_venue() {
    let app = TestApp::start().await;
    let shop = app.shop(10, 1_000, json!({})).await;
    let path = format!("/v1/events/{}", shop.event_id);
    let event = app.call(Method::GET, &path, None, None).await.body;
    assert_eq!(event["address"], json!(null));

    let big_sight = json!({
        "country": "jp",
        "region": "東京都",
        "locality": "江東区",
        "postal_code": "135-0063",
        "street": "有明3-11-1",
        "latitude": 35.6298,
        "longitude": 139.7942,
    });
    let located = app
        .call(
            Method::PATCH,
            &path,
            Some(&shop.organizer),
            Some(json!({ "version": event["version"], "address": big_sight })),
        )
        .await;
    assert_eq!(located.status, StatusCode::OK, "{:?}", located.body);
    assert_eq!(located.body["venue"], event["venue"]);
    assert_eq!(located.body["address"]["country"], "JP");
    assert_eq!(located.body["address"]["street"], "有明3-11-1");
    assert_eq!(located.body["address"]["latitude"], 35.6298);
    assert_eq!(
        app.call(Method::GET, &path, None, None).await.body,
        located.body
    );

    // A patch without `address` keeps it.
    let retitled = app
        .call(
            Method::PATCH,
            &path,
            Some(&shop.organizer),
            Some(json!({ "version": located.body["version"], "title": "C111" })),
        )
        .await;
    assert_eq!(retitled.body["address"], located.body["address"]);

    let half_a_point = app
        .call(
            Method::PATCH,
            &path,
            Some(&shop.organizer),
            Some(json!({
                "version": retitled.body["version"],
                "address": { "country": "JP", "street": "有明3-11-1", "latitude": 35.6 },
            })),
        )
        .await;
    assert_eq!(half_a_point.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        half_a_point.body["type"],
        "urn:kippu:problem:invalid-request"
    );
    let misspelt = app
        .call(
            Method::PATCH,
            &path,
            Some(&shop.organizer),
            Some(json!({
                "version": retitled.body["version"],
                "address": { "country": "JP", "street": "有明3-11-1", "zip": "135-0063" },
            })),
        )
        .await;
    assert_eq!(misspelt.body["type"], "urn:kippu:problem:invalid-json");

    // `null` removes the address: the event went online.
    let online = app
        .call(
            Method::PATCH,
            &path,
            Some(&shop.organizer),
            Some(json!({
                "version": retitled.body["version"],
                "venue": "Online",
                "address": null,
            })),
        )
        .await;
    assert_eq!(online.status, StatusCode::OK, "{:?}", online.body);
    assert_eq!(online.body["address"], json!(null));

    // A full update states the address too; leaving it out means none.
    let mut moved = online.body.clone();
    moved["venue"] = json!("Makuhari Messe");
    moved["address"] = json!({ "country": "JP", "street": "中瀬2-1" });
    let moved = app
        .call(Method::PUT, &path, Some(&shop.organizer), Some(moved))
        .await;
    assert_eq!(moved.status, StatusCode::OK, "{:?}", moved.body);
    assert_eq!(moved.body["address"]["region"], json!(null));
    let listed = app.call(Method::GET, "/v1/events", None, None).await.body;
    assert_eq!(listed["items"][0]["address"], moved.body["address"]);
}
