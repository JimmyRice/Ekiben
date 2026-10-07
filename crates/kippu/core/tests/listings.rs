//! Listings: `{items, next_cursor}` bodies, cursors, filters and sort orders.
#![allow(
    clippy::unwrap_used,
    reason = "test helpers: a failure is the test failing"
)]

mod support;

use axum::http::{Method, StatusCode};
use kippu_domain::Duration;
use serde_json::{Value, json};
use support::{TestApp, uuid_suffix};

/// Follows `next_cursor` from `path` to the end, returning the ids in the order listed.
async fn walk(app: &TestApp, token: Option<&str>, path: &str) -> Vec<String> {
    let separator = if path.contains('?') { '&' } else { '?' };
    let mut ids = Vec::new();
    let mut next = path.to_owned();
    loop {
        let page = app.call(Method::GET, &next, token, None).await;
        assert_eq!(page.status, StatusCode::OK, "{next}: {:?}", page.body);
        for item in page.body["items"].as_array().unwrap() {
            ids.push(item["id"].as_str().unwrap().to_owned());
        }
        match page.body["next_cursor"].as_str() {
            Some(cursor) => next = format!("{path}{separator}cursor={cursor}"),
            None => return ids,
        }
    }
}

/// Creates an event in the organization of `shop`'s event, published unless `draft`.
async fn event(
    app: &TestApp,
    organizer: &str,
    organization_id: &str,
    starts_in_days: i64,
    country: Option<&str>,
    draft: bool,
) -> String {
    let starts_at = app.now() + Duration::days(starts_in_days);
    let address = country.map(|country| json!({ "country": country, "street": "1-1" }));
    let created = app
        .call(
            Method::POST,
            &format!("/v1/organizations/{organization_id}/events"),
            Some(organizer),
            Some(json!({
                "slug": format!("event-{}", uuid_suffix()),
                "title": "Convention",
                "venue": "Hall",
                "address": address,
                "starts_at": starts_at.to_string(),
                "ends_at": (starts_at + Duration::days(1)).to_string(),
            })),
        )
        .await;
    assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);
    let id = created.body["id"].as_str().unwrap().to_owned();
    if !draft {
        let published = app
            .call(
                Method::PATCH,
                &format!("/v1/events/{id}"),
                Some(organizer),
                Some(json!({ "version": created.body["version"], "status": "published" })),
            )
            .await;
        assert_eq!(published.status, StatusCode::OK, "{:?}", published.body);
    }
    id
}

fn problem_type(reply: &Value) -> &str {
    reply["type"].as_str().unwrap_or_default()
}

#[tokio::test]
async fn events_are_filtered_sorted_and_paged_with_cursors() {
    let app = TestApp::start().await;
    let shop = app.shop(10, 1_000, json!({})).await; // published, starts in 30 days
    let organizer = shop.organizer.as_str();
    let organization = app
        .call(
            Method::GET,
            &format!("/v1/events/{}", shop.event_id),
            None,
            None,
        )
        .await
        .body["organization_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let japan = event(&app, organizer, &organization, 10, Some("JP"), false).await;
    let taiwan = event(&app, organizer, &organization, 20, Some("TW"), false).await;
    let draft = event(&app, organizer, &organization, 5, Some("JP"), true).await;
    let mine = format!("/v1/organizations/{organization}/events");

    // The public sees published events only, earliest start first by default, in any page size.
    let by_start = vec![japan.clone(), taiwan.clone(), shop.event_id.clone()];
    assert_eq!(walk(&app, None, "/v1/events").await, by_start);
    assert_eq!(walk(&app, None, "/v1/events?limit=1").await, by_start);
    let latest_first: Vec<String> = by_start.iter().rev().cloned().collect();
    assert_eq!(
        walk(&app, None, "/v1/events?sort=-starts_at&limit=2").await,
        latest_first
    );
    let first = app
        .call(Method::GET, "/v1/events?limit=3", None, None)
        .await;
    assert_eq!(first.body["next_cursor"], json!(null), "a full last page");
    let created_last_first = walk(&app, None, "/v1/events?sort=-created_at&limit=2").await;
    assert_eq!(
        created_last_first,
        vec![taiwan.clone(), japan.clone(), shop.event_id.clone()]
    );

    // Filters combine with each other and with the visibility of the listing.
    assert_eq!(
        walk(&app, None, "/v1/events?country=jp").await,
        vec![japan.clone()]
    );
    let token = Some(organizer);
    assert_eq!(
        walk(
            &app,
            token,
            &format!("{mine}?country=JP&sort=-starts_at&limit=1")
        )
        .await,
        vec![japan.clone(), draft.clone()]
    );
    assert_eq!(
        walk(&app, token, &format!("{mine}?status=draft")).await,
        vec![draft.clone()]
    );
    let in_two_weeks = app.now() + Duration::days(14);
    assert_eq!(
        walk(&app, token, &format!("{mine}?starts_before={in_two_weeks}")).await,
        vec![draft.clone(), japan.clone()]
    );
    assert_eq!(
        walk(&app, None, &format!("/v1/events?ends_from={in_two_weeks}")).await,
        vec![taiwan.clone(), shop.event_id.clone()]
    );

    // A cursor resumes only the listing and order it came from.
    let page = app
        .call(Method::GET, "/v1/events?limit=1", None, None)
        .await;
    let cursor = page.body["next_cursor"].as_str().unwrap();
    for path in [
        format!("/v1/events?sort=-starts_at&cursor={cursor}"),
        format!("/v1/me/tickets?cursor={cursor}"),
        "/v1/events?cursor=not-a-cursor".to_owned(),
    ] {
        let reply = app.call(Method::GET, &path, Some(organizer), None).await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{path}");
        assert_eq!(
            problem_type(&reply.body),
            "urn:kippu:problem:invalid-parameter"
        );
    }
    let same_order = app
        .call(
            Method::GET,
            &format!("/v1/events?sort=starts_at&cursor={cursor}"),
            None,
            None,
        )
        .await;
    assert_eq!(same_order.body["items"][0]["id"], json!(taiwan));

    // Unknown or malformed parameters are errors, not ignored.
    for path in [
        "/v1/events?contry=JP",
        "/v1/events?country=JPN",
        "/v1/events?sort=title",
    ] {
        let reply = app.call(Method::GET, path, None, None).await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{path}");
        assert_eq!(
            problem_type(&reply.body),
            "urn:kippu:problem:invalid-parameter"
        );
    }
}

#[tokio::test]
async fn a_buyers_tickets_and_reservations_are_paged() {
    let app = TestApp::start().await;
    let shop = app.shop(10, 0, json!({})).await;
    let buyer = app.buyer().await;
    for _ in 0..2 {
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
    }

    let all = app
        .call(Method::GET, "/v1/me/tickets", Some(&buyer), None)
        .await;
    assert_eq!(all.body["items"].as_array().unwrap().len(), 2);
    assert_eq!(all.body["next_cursor"], json!(null));
    let one_by_one = walk(&app, Some(&buyer), "/v1/me/tickets?limit=1").await;
    let ids: Vec<&str> = all.body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|ticket| ticket["id"].as_str().unwrap())
        .collect();
    assert_eq!(one_by_one, ids);
    assert_eq!(
        walk(&app, Some(&buyer), "/v1/me/reservations?limit=1")
            .await
            .len(),
        2
    );

    // Lists that never need pages still come as listings.
    let sales = app
        .call(
            Method::GET,
            &format!("/v1/events/{}/sales", shop.event_id),
            None,
            None,
        )
        .await;
    assert_eq!(sales.body["items"].as_array().unwrap().len(), 1);
    assert_eq!(sales.body["next_cursor"], json!(null));
}
