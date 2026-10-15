//! Who sees a sale, through the HTTP API: public sales for everyone, invited-only sales for
//! the accounts on their list, and private sales whose free tickets organizers grant.
#![allow(
    clippy::unwrap_used,
    reason = "test helpers: a failure is the test failing"
)]

mod support;

use axum::http::{Method, StatusCode};
use serde_json::json;
use support::{Reply, Shop, TestApp, uuid_suffix};

async fn account_id(app: &TestApp, token: &str) -> String {
    let me = app.call(Method::GET, "/v1/me", Some(token), None).await;
    me.body["account"]["id"].as_str().unwrap().to_owned()
}

/// The ids of the sales of the shop's event the caller sees.
async fn listed_sales(app: &TestApp, shop: &Shop, token: Option<&str>) -> Vec<String> {
    let sales = app
        .call(
            Method::GET,
            &format!("/v1/events/{}/sales", shop.event_id),
            token,
            None,
        )
        .await;
    assert_eq!(sales.status, StatusCode::OK, "{:?}", sales.body);
    sales.body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|sale| sale["id"].as_str().unwrap().to_owned())
        .collect()
}

async fn invite(app: &TestApp, shop: &Shop, account: &str) -> Reply {
    app.call(
        Method::PUT,
        &format!("/v1/sales/{}/invitations/{account}", shop.sale_id),
        Some(&shop.organizer),
        None,
    )
    .await
}

async fn grant(
    app: &TestApp,
    shop: &Shop,
    token: &str,
    account: &str,
    quantity: u32,
    key: &str,
) -> Reply {
    app.call_with(
        Method::POST,
        &format!("/v1/sales/{}/grants", shop.sale_id),
        Some(token),
        Some(json!({
            "account_id": account,
            "items": [{ "ticket_type_id": shop.ticket_type_id, "quantity": quantity }],
        })),
        &[("idempotency-key", key)],
    )
    .await
}

fn kind(reply: &Reply) -> &str {
    reply.body["type"].as_str().unwrap_or_default()
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "an invited-only sale from the first invitation to the last, told in order"
)]
async fn invited_sales_are_seen_and_bought_by_invited_accounts_only() {
    let app = TestApp::start().await;
    let shop = app.shop(10, 500, json!({ "audience": "invited" })).await;
    assert_eq!(
        app.call(
            Method::GET,
            &format!("/v1/sales/{}", shop.sale_id),
            Some(&shop.organizer),
            None
        )
        .await
        .body["audience"],
        "invited"
    );
    let cosplayer = app.buyer().await;
    let cosplayer_id = account_id(&app, &cosplayer).await;
    let stranger = app.buyer().await;

    let added = invite(&app, &shop, &cosplayer_id).await;
    assert_eq!(added.status, StatusCode::CREATED, "{:?}", added.body);
    assert_eq!(added.body["account_id"], cosplayer_id.as_str());
    let again = invite(&app, &shop, &cosplayer_id).await;
    assert_eq!(again.status, StatusCode::OK);
    assert_eq!(again.body, added.body, "invited once, at the first time");

    // Only the organizers and the invited see it; to everyone else it does not exist.
    assert_eq!(
        listed_sales(&app, &shop, Some(&shop.organizer)).await,
        [shop.sale_id.as_str()]
    );
    assert_eq!(
        listed_sales(&app, &shop, Some(&cosplayer)).await,
        [shop.sale_id.as_str()]
    );
    assert_eq!(
        listed_sales(&app, &shop, Some(&stranger)).await,
        Vec::<String>::new()
    );
    assert_eq!(listed_sales(&app, &shop, None).await, Vec::<String>::new());
    let sale_path = format!("/v1/sales/{}", shop.sale_id);
    assert_eq!(
        app.call(Method::GET, &sale_path, Some(&cosplayer), None)
            .await
            .status,
        StatusCode::OK
    );
    for token in [Some(stranger.as_str()), None] {
        assert_eq!(
            app.call(Method::GET, &sale_path, token, None).await.status,
            StatusCode::NOT_FOUND
        );
    }
    let ticket_type = format!("/v1/ticket-types/{}", shop.ticket_type_id);
    assert_eq!(
        app.call(Method::GET, &ticket_type, Some(&stranger), None)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.purchase(&stranger, &shop, 1, &uuid_suffix())
            .await
            .status,
        StatusCode::NOT_FOUND
    );

    let reservation = app.reserve(&cosplayer, &shop, 1).await;
    assert_eq!(reservation["status"], "reserved");

    // Taken off the list while a request waits: the worker turns it away.
    let queued = app.purchase(&cosplayer, &shop, 1, &uuid_suffix()).await;
    assert_eq!(queued.status, StatusCode::ACCEPTED, "{:?}", queued.body);
    let removed = app
        .call(
            Method::DELETE,
            &format!("/v1/sales/{}/invitations/{cosplayer_id}", shop.sale_id),
            Some(&shop.organizer),
            None,
        )
        .await;
    assert_eq!(removed.status, StatusCode::NO_CONTENT, "{:?}", removed.body);
    app.drain("purchases").await;
    let request = app
        .call(
            Method::GET,
            &format!(
                "/v1/purchase-requests/{}",
                queued.body["id"].as_str().unwrap()
            ),
            Some(&cosplayer),
            None,
        )
        .await;
    assert_eq!(request.body["status"], "rejected");
    assert_eq!(request.body["reason"], "not_invited");
    assert_eq!(
        app.call(Method::GET, &sale_path, Some(&cosplayer), None)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    // What it bought stays its own.
    let held = app
        .call(
            Method::GET,
            &format!("/v1/reservations/{}", reservation["id"].as_str().unwrap()),
            Some(&cosplayer),
            None,
        )
        .await;
    assert_eq!(held.status, StatusCode::OK);
    let gone = app
        .call(
            Method::DELETE,
            &format!("/v1/sales/{}/invitations/{cosplayer_id}", shop.sale_id),
            Some(&shop.organizer),
            None,
        )
        .await;
    assert_eq!(gone.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "a list kept on a public sale, then the sale turned invited-only"
)]
async fn invitation_lists_are_kept_by_organizers() {
    let app = TestApp::start().await;
    let shop = app.shop(10, 500, json!({})).await;
    let mut invited = Vec::new();
    for _ in 0..3 {
        let buyer = app.buyer().await;
        let id = account_id(&app, &buyer).await;
        // A public sale keeps its list for when it is invited-only.
        assert_eq!(invite(&app, &shop, &id).await.status, StatusCode::CREATED);
        invited.push(id);
    }
    let unknown = app
        .call(
            Method::PUT,
            &format!(
                "/v1/sales/{}/invitations/{}",
                shop.sale_id,
                uuid::Uuid::now_v7()
            ),
            Some(&shop.organizer),
            None,
        )
        .await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);

    let mut listed = Vec::new();
    let mut path = format!("/v1/sales/{}/invitations?limit=2", shop.sale_id);
    loop {
        let page = app
            .call(Method::GET, &path, Some(&shop.organizer), None)
            .await;
        assert_eq!(page.status, StatusCode::OK, "{:?}", page.body);
        for invitation in page.body["items"].as_array().unwrap() {
            listed.push(invitation["account_id"].as_str().unwrap().to_owned());
        }
        match page.body["next_cursor"].as_str() {
            Some(cursor) => {
                path = format!(
                    "/v1/sales/{}/invitations?limit=2&cursor={cursor}",
                    shop.sale_id
                );
            }
            None => break,
        }
    }
    let mut expected = invited.clone();
    expected.sort();
    listed.sort();
    assert_eq!(listed, expected);

    // Buyers and other organizers keep nobody's list.
    let buyer = app.buyer().await;
    let other = app.shop(10, 500, json!({})).await;
    for token in [&buyer, &other.organizer] {
        let read = app
            .call(
                Method::GET,
                &format!("/v1/sales/{}/invitations", shop.sale_id),
                Some(token),
                None,
            )
            .await;
        assert!(
            matches!(read.status, StatusCode::FORBIDDEN | StatusCode::NOT_FOUND),
            "{:?}",
            read.status
        );
        let added = app
            .call(
                Method::PUT,
                &format!("/v1/sales/{}/invitations/{}", shop.sale_id, invited[0]),
                Some(token),
                None,
            )
            .await;
        assert!(matches!(
            added.status,
            StatusCode::FORBIDDEN | StatusCode::NOT_FOUND
        ));
    }

    // Switching to invited-only and back is a plain edit.
    let patched = app
        .call(
            Method::PATCH,
            &format!("/v1/sales/{}", shop.sale_id),
            Some(&shop.organizer),
            Some(json!({ "version": 1, "audience": "invited" })),
        )
        .await;
    assert_eq!(patched.status, StatusCode::OK, "{:?}", patched.body);
    assert_eq!(patched.body["audience"], "invited");
    assert_eq!(
        listed_sales(&app, &shop, Some(&buyer)).await,
        Vec::<String>::new()
    );
    // A full update without the field leaves it as it is.
    let mut replaced = patched.body.clone();
    replaced.as_object_mut().unwrap().remove("audience");
    let replaced = app
        .call(
            Method::PUT,
            &format!("/v1/sales/{}", shop.sale_id),
            Some(&shop.organizer),
            Some(replaced),
        )
        .await;
    assert_eq!(replaced.status, StatusCode::OK, "{:?}", replaced.body);
    assert_eq!(replaced.body["audience"], "invited");
    let private = app
        .call(
            Method::PATCH,
            &format!("/v1/sales/{}", shop.sale_id),
            Some(&shop.organizer),
            Some(json!({ "version": 3, "audience": "private" })),
        )
        .await;
    assert_eq!(
        private.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{:?}",
        private.body
    );
}

#[tokio::test]
async fn private_sales_are_seen_by_organizers_only_and_never_sold() {
    let app = TestApp::start().await;
    let shop = app.shop(10, 0, json!({ "audience": "private" })).await;
    let buyer = app.buyer().await;
    let sale_path = format!("/v1/sales/{}", shop.sale_id);

    assert_eq!(
        listed_sales(&app, &shop, Some(&buyer)).await,
        Vec::<String>::new()
    );
    assert_eq!(
        listed_sales(&app, &shop, Some(&shop.organizer)).await,
        [shop.sale_id.as_str()]
    );
    assert_eq!(
        app.call(Method::GET, &sale_path, Some(&buyer), None)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.purchase(&buyer, &shop, 1, &uuid_suffix()).await.status,
        StatusCode::NOT_FOUND
    );
    let organizer_buys = app
        .purchase(&shop.organizer, &shop, 1, &uuid_suffix())
        .await;
    assert_eq!(organizer_buys.status, StatusCode::CONFLICT);
    assert_eq!(
        kind(&organizer_buys),
        "urn:kippu:problem:wrong-sale-audience"
    );
    let listed = app
        .call(
            Method::GET,
            &format!("{sale_path}/invitations"),
            Some(&shop.organizer),
            None,
        )
        .await;
    assert_eq!(kind(&listed), "urn:kippu:problem:wrong-sale-audience");

    // Private for good; free, unqueued and paid by nobody.
    let public = app
        .call(
            Method::PATCH,
            &sale_path,
            Some(&shop.organizer),
            Some(json!({ "version": 1, "audience": "public" })),
        )
        .await;
    assert_eq!(
        public.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{:?}",
        public.body
    );
    let priced = app
        .call(
            Method::PATCH,
            &format!("/v1/ticket-types/{}", shop.ticket_type_id),
            Some(&shop.organizer),
            Some(json!({ "version": 1, "price": { "amount_minor": 500, "currency": "JPY" } })),
        )
        .await;
    assert_eq!(
        priced.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{:?}",
        priced.body
    );
    for setting in [
        json!({ "accepted_attestors": [kippu_domain::AttestorId::MANUAL] }),
        json!({ "admission": { "mode": "waiting_room", "admit_per_tick": 10, "max_backlog": 100 } }),
    ] {
        let mut patch = json!({ "version": 1 });
        patch
            .as_object_mut()
            .unwrap()
            .extend(setting.as_object().unwrap().clone());
        let refused = app
            .call(
                Method::PATCH,
                &sale_path,
                Some(&shop.organizer),
                Some(patch),
            )
            .await;
        assert_eq!(
            refused.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{:?}",
            refused.body
        );
    }
}

#[tokio::test]
async fn organizers_grant_private_tickets_once_per_key() {
    let app = TestApp::start().await;
    let shop = app.shop(3, 0, json!({ "audience": "private" })).await;
    let guest = app.buyer().await;
    let guest_id = account_id(&app, &guest).await;

    let granted = grant(
        &app,
        &shop,
        &shop.organizer,
        &guest_id,
        2,
        "guest-of-honour",
    )
    .await;
    assert_eq!(granted.status, StatusCode::CREATED, "{:?}", granted.body);
    assert_eq!(granted.body["status"], "issued");
    assert_eq!(granted.body["total"]["amount_minor"], 0);
    let reservation_id = granted.body["id"].as_str().unwrap().to_owned();
    assert_eq!(
        granted.headers["location"].to_str().unwrap(),
        format!("/v1/reservations/{reservation_id}")
    );

    // The guest holds the tickets, signed bytes included, like bought ones.
    let tickets = app
        .call(Method::GET, "/v1/me/tickets", Some(&guest), None)
        .await;
    let tickets = tickets.body["items"].as_array().unwrap().clone();
    assert_eq!(tickets.len(), 2);
    assert!(tickets.iter().all(|ticket| ticket["ticket"].is_string()));
    assert!(
        tickets
            .iter()
            .all(|ticket| ticket["ticket_type_name"] == "Day 1")
    );

    // The same key grants nothing more; another basket under it is refused.
    let again = grant(
        &app,
        &shop,
        &shop.organizer,
        &guest_id,
        2,
        "guest-of-honour",
    )
    .await;
    assert_eq!(again.status, StatusCode::OK, "{:?}", again.body);
    assert_eq!(again.body["id"], reservation_id.as_str());
    let reused = grant(
        &app,
        &shop,
        &shop.organizer,
        &guest_id,
        1,
        "guest-of-honour",
    )
    .await;
    assert_eq!(kind(&reused), "urn:kippu:problem:idempotency-key-reused");
    let keyless = app
        .call(
            Method::POST,
            &format!("/v1/sales/{}/grants", shop.sale_id),
            Some(&shop.organizer),
            Some(json!({ "account_id": guest_id, "items": [{ "ticket_type_id": shop.ticket_type_id, "quantity": 1 }] })),
        )
        .await;
    assert_eq!(keyless.status, StatusCode::UNPROCESSABLE_ENTITY);

    // The per-account limit (2) and the stock (3) hold as for purchases.
    let over_limit = grant(&app, &shop, &shop.organizer, &guest_id, 1, &uuid_suffix()).await;
    assert_eq!(over_limit.status, StatusCode::CONFLICT);
    assert_eq!(kind(&over_limit), "urn:kippu:problem:limit-exceeded");
    let exhibitor = account_id(&app, &app.buyer().await).await;
    let sold_out = grant(&app, &shop, &shop.organizer, &exhibitor, 2, &uuid_suffix()).await;
    assert_eq!(kind(&sold_out), "urn:kippu:problem:sold-out");
    let last = grant(&app, &shop, &shop.organizer, &exhibitor, 1, &uuid_suffix()).await;
    assert_eq!(last.status, StatusCode::CREATED, "{:?}", last.body);
    let stock = app
        .call(
            Method::GET,
            &format!("/v1/sales/{}/inventory", shop.sale_id),
            Some(&shop.organizer),
            None,
        )
        .await;
    assert_eq!(stock.body["items"][0]["sold"], 3, "{:?}", stock.body);

    // Organizers see the grants among the event's reservations.
    let reservations = app
        .call(
            Method::GET,
            &format!("/v1/events/{}/reservations", shop.event_id),
            Some(&shop.organizer),
            None,
        )
        .await;
    assert_eq!(reservations.body["items"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn grants_go_to_open_accounts_of_private_sales_from_their_organizers() {
    let app = TestApp::start().await;
    let shop = app.shop(10, 0, json!({ "audience": "private" })).await;
    let guest = app.buyer().await;
    let guest_id = account_id(&app, &guest).await;

    // Nobody else grants: the sale is hidden from buyers, out of other organizers' reach.
    let other = app.shop(10, 0, json!({})).await;
    let by_buyer = grant(&app, &shop, &guest, &guest_id, 1, &uuid_suffix()).await;
    assert_eq!(by_buyer.status, StatusCode::NOT_FOUND);
    let by_stranger = grant(&app, &shop, &other.organizer, &guest_id, 1, &uuid_suffix()).await;
    assert_eq!(by_stranger.status, StatusCode::NOT_FOUND);
    // Only private sales' tickets are granted.
    let public = grant(&app, &other, &other.organizer, &guest_id, 1, &uuid_suffix()).await;
    assert_eq!(kind(&public), "urn:kippu:problem:wrong-sale-audience");

    let unknown = grant(
        &app,
        &shop,
        &shop.organizer,
        &uuid::Uuid::now_v7().to_string(),
        1,
        &uuid_suffix(),
    )
    .await;
    assert_eq!(
        unknown.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{:?}",
        unknown.body
    );
    let denied = app
        .call(
            Method::POST,
            &format!("/v1/events/{}/denials", shop.event_id),
            Some(&shop.organizer),
            Some(json!({ "subject": { "kind": "account", "id": guest_id } })),
        )
        .await;
    assert_eq!(denied.status, StatusCode::CREATED, "{:?}", denied.body);
    let refused = grant(&app, &shop, &shop.organizer, &guest_id, 1, &uuid_suffix()).await;
    assert_eq!(kind(&refused), "urn:kippu:problem:account-denied");

    // Not while the event is cancelled.
    let event = app
        .call(
            Method::GET,
            &format!("/v1/events/{}", shop.event_id),
            Some(&shop.organizer),
            None,
        )
        .await;
    let cancelled = app
        .call(
            Method::PATCH,
            &format!("/v1/events/{}", shop.event_id),
            Some(&shop.organizer),
            Some(json!({ "version": event.body["version"], "status": "cancelled" })),
        )
        .await;
    assert_eq!(cancelled.status, StatusCode::OK, "{:?}", cancelled.body);
    let other_guest = account_id(&app, &app.buyer().await).await;
    let closed = grant(
        &app,
        &shop,
        &shop.organizer,
        &other_guest,
        1,
        &uuid_suffix(),
    )
    .await;
    assert_eq!(kind(&closed), "urn:kippu:problem:sale-closed");
}

#[tokio::test]
async fn granted_tickets_are_refunded_and_returned_like_free_ones() {
    let app = TestApp::start().await;
    let shop = app.shop(1, 0, json!({ "audience": "private" })).await;
    let guest = app.buyer().await;
    let guest_id = account_id(&app, &guest).await;
    let granted = grant(&app, &shop, &shop.organizer, &guest_id, 1, &uuid_suffix()).await;
    assert_eq!(granted.status, StatusCode::CREATED, "{:?}", granted.body);
    let reservation_id = granted.body["id"].as_str().unwrap();

    let refunded = app
        .call(
            Method::POST,
            &format!("/v1/reservations/{reservation_id}/refunds"),
            Some(&shop.organizer),
            Some(json!({})),
        )
        .await;
    assert!(refunded.status.is_success(), "{:?}", refunded.body);
    assert_eq!(refunded.body["status"], "completed", "{:?}", refunded.body);

    // The ticket came back to stock: someone else can have it.
    let next = account_id(&app, &app.buyer().await).await;
    let regranted = grant(&app, &shop, &shop.organizer, &next, 1, &uuid_suffix()).await;
    assert_eq!(
        regranted.status,
        StatusCode::CREATED,
        "{:?}",
        regranted.body
    );
}

#[tokio::test]
async fn sales_are_public_unless_said_otherwise() {
    let app = TestApp::start().await;
    let shop = app.shop(10, 500, json!({})).await;
    let sale = app
        .call(
            Method::GET,
            &format!("/v1/sales/{}", shop.sale_id),
            None,
            None,
        )
        .await;
    assert_eq!(sale.body["audience"], "public");
    let unknown = app
        .call(
            Method::PATCH,
            &format!("/v1/sales/{}", shop.sale_id),
            Some(&shop.organizer),
            Some(json!({ "version": 1, "audience": "secret" })),
        )
        .await;
    assert!(unknown.status.is_client_error(), "{:?}", unknown.body);
}
