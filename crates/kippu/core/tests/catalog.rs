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
async fn a_sale_sells_in_one_currency() {
    let app = TestApp::start().await;
    let shop = app.shop(10, 1_000, json!({})).await;
    let now = app.now();
    let ticket_type = |currency: &str| {
        json!({
            "name": "Day 2",
            "price": { "amount_minor": 100, "currency": currency },
            "capacity": 10,
            "valid_from": (now + kippu_domain::Duration::days(30)).to_string(),
            "valid_until": (now + kippu_domain::Duration::days(31)).to_string(),
        })
    };
    let path = format!("/v1/sales/{}/ticket-types", shop.sale_id);

    let yuan = app
        .call(
            Method::POST,
            &path,
            Some(&shop.organizer),
            Some(ticket_type("CNY")),
        )
        .await;
    assert_eq!(
        yuan.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{:?}",
        yuan.body
    );
    assert_eq!(yuan.body["type"], "urn:kippu:problem:invalid-request");
    let yen = app
        .call(
            Method::POST,
            &path,
            Some(&shop.organizer),
            Some(ticket_type("JPY")),
        )
        .await;
    assert_eq!(yen.status, StatusCode::CREATED, "{:?}", yen.body);

    // Moving one of two ticket types to another currency is refused too.
    let moved = app
        .call(
            Method::PATCH,
            &format!("/v1/ticket-types/{}", shop.ticket_type_id),
            Some(&shop.organizer),
            Some(json!({
                "version": 1,
                "price": { "amount_minor": 100, "currency": "CNY" },
            })),
        )
        .await;
    assert_eq!(
        moved.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{:?}",
        moved.body
    );
}

#[tokio::test]
async fn a_ticket_type_is_visible_wherever_its_event_is() {
    let app = TestApp::start().await;
    let shop = app.shop(10, 1_000, json!({})).await;
    let path = format!("/v1/ticket-types/{}", shop.ticket_type_id);

    let fetched = app.call(Method::GET, &path, None, None).await;
    assert_eq!(fetched.status, StatusCode::OK, "{:?}", fetched.body);
    let sale_path = format!("/v1/sales/{}", shop.sale_id);
    let sale = app.call(Method::GET, &sale_path, None, None).await.body;
    assert_eq!(fetched.body, sale["ticket_types"][0]);
    assert_eq!(fetched.body["available"], 10);

    // Its version is the one an edit expects.
    let renamed = app
        .call(
            Method::PATCH,
            &path,
            Some(&shop.organizer),
            Some(json!({ "version": fetched.body["version"], "name": "Day 1 (East)" })),
        )
        .await;
    assert_eq!(renamed.status, StatusCode::OK, "{:?}", renamed.body);
    assert_eq!(
        app.call(Method::GET, &path, None, None).await.body["name"],
        "Day 1 (East)"
    );

    let unknown = app
        .call(
            Method::GET,
            "/v1/ticket-types/01994a3c-7d00-7000-8000-000000000001",
            None,
            None,
        )
        .await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);
    assert_eq!(unknown.body["type"], "urn:kippu:problem:not-found");

    // A draft's ticket types are hidden like the draft itself.
    let event_path = format!("/v1/events/{}", shop.event_id);
    let event = app.call(Method::GET, &event_path, None, None).await.body;
    let drafted = app
        .call(
            Method::PATCH,
            &event_path,
            Some(&shop.organizer),
            Some(json!({ "version": event["version"], "status": "draft" })),
        )
        .await;
    assert_eq!(drafted.status, StatusCode::OK, "{:?}", drafted.body);
    let stranger = app.buyer().await;
    for token in [None, Some(stranger.as_str())] {
        let hidden = app.call(Method::GET, &path, token, None).await;
        assert_eq!(hidden.status, StatusCode::NOT_FOUND, "{:?}", hidden.body);
    }
    let own = app
        .call(Method::GET, &path, Some(&shop.organizer), None)
        .await;
    assert_eq!(own.status, StatusCode::OK, "{:?}", own.body);
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
