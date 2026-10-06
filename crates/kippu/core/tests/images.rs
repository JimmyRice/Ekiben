//! Event images: uploads go to the object store, the database keeps the gallery.
#![allow(
    clippy::unwrap_used,
    clippy::missing_panics_doc,
    reason = "test helpers: a failure is the test failing"
)]

mod support;

use std::sync::Arc;

use axum::http::{Method, StatusCode};
use object_store::ObjectStoreExt;
use object_store::memory::InMemory;
use object_store::path::Path;
use serde_json::{Value, json};
use support::{Reply, Shop, TestApp};

/// The smallest thing that sniffs as a PNG.
const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR not really an image";
const JPEG: &[u8] = &[0xff, 0xd8, 0xff, 0xe0, 0, 0x10, b'J', b'F', b'I', b'F'];

async fn app_with(
    store: Arc<InMemory>,
    configure: impl FnOnce(&mut kippu_core::Config),
) -> TestApp {
    TestApp::start_custom(Vec::new(), configure, move |kippu| {
        kippu.object_store(store)
    })
    .await
}

async fn upload(
    app: &TestApp,
    shop: &Shop,
    token: &str,
    content_type: &str,
    bytes: &[u8],
) -> Reply {
    let bearer = format!("Bearer {token}");
    app.call_raw(
        Method::POST,
        &format!("/v1/events/{}/images", shop.event_id),
        &[("authorization", &bearer), ("content-type", content_type)],
        bytes.to_vec(),
    )
    .await
}

#[tokio::test]
async fn an_uploaded_image_is_stored_listed_and_served() {
    let store = Arc::new(InMemory::new());
    let app = app_with(store.clone(), |_| {}).await;
    let shop = app.shop(10, 1_000, json!({})).await;

    let uploaded = upload(&app, &shop, &shop.organizer, "image/png", PNG).await;
    assert_eq!(uploaded.status, StatusCode::CREATED, "{:?}", uploaded.body);
    let image_id = uploaded.body["id"].as_str().unwrap().to_owned();
    assert_eq!(uploaded.body["format"], "image/png");
    assert_eq!(uploaded.body["size_bytes"], PNG.len());
    let url = format!("/v1/events/{}/images/{image_id}", shop.event_id);
    assert_eq!(uploaded.body["url"], url);

    let key = format!("events/{}/images/{image_id}.png", shop.event_id);
    let stored = store
        .get(&Path::from(key))
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_eq!(stored.as_ref(), PNG);

    let listed = app
        .call(
            Method::GET,
            &format!("/v1/events/{}/images", shop.event_id),
            None,
            None,
        )
        .await;
    assert_eq!(listed.body.as_array().unwrap().len(), 1);
    let served = app.call(Method::GET, &url, None, None).await;
    assert_eq!(served.status, StatusCode::OK);
    assert_eq!(served.headers["content-type"], "image/png");
    assert!(
        served.headers["cache-control"]
            .to_str()
            .unwrap()
            .contains("immutable")
    );
    assert_eq!(served.bytes, PNG);
}

#[tokio::test]
async fn only_real_images_of_bounded_size_and_number_are_accepted() {
    let app = app_with(Arc::new(InMemory::new()), |config| {
        config.images.max_bytes = 64;
        config.images.max_per_event = 2;
    })
    .await;
    let shop = app.shop(10, 1_000, json!({})).await;

    for (content_type, bytes) in [
        ("image/png", JPEG),
        (
            "image/svg+xml",
            b"<svg xmlns='http://www.w3.org/2000/svg'/>".as_slice(),
        ),
        ("text/plain", PNG),
    ] {
        let refused = upload(&app, &shop, &shop.organizer, content_type, bytes).await;
        assert_eq!(
            refused.status,
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "{content_type}"
        );
        assert_eq!(
            refused.body["type"],
            "urn:kippu:problem:unsupported-media-type"
        );
    }
    let mut large = PNG.to_vec();
    large.resize(65, 0);
    let too_large = upload(&app, &shop, &shop.organizer, "image/png", &large).await;
    assert_eq!(too_large.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        too_large.body["type"],
        "urn:kippu:problem:payload-too-large"
    );

    let buyer = app.buyer().await;
    let forbidden = upload(&app, &shop, &buyer, "image/png", PNG).await;
    assert_eq!(forbidden.status, StatusCode::FORBIDDEN);

    for _ in 0..2 {
        let accepted = upload(&app, &shop, &shop.organizer, "image/jpeg", JPEG).await;
        assert_eq!(accepted.status, StatusCode::CREATED, "{:?}", accepted.body);
    }
    let third = upload(&app, &shop, &shop.organizer, "image/jpeg", JPEG).await;
    assert_eq!(third.status, StatusCode::CONFLICT);
    assert_eq!(third.body["type"], "urn:kippu:problem:too-many-images");
}

#[tokio::test]
async fn images_can_be_reordered_and_deleted() {
    let store = Arc::new(InMemory::new());
    let app = app_with(store.clone(), |_| {}).await;
    let shop = app.shop(10, 1_000, json!({})).await;
    let mut ids = Vec::new();
    for _ in 0..3 {
        let uploaded = upload(&app, &shop, &shop.organizer, "image/png", PNG).await;
        ids.push(uploaded.body["id"].as_str().unwrap().to_owned());
    }
    let order_path = format!("/v1/events/{}/images/order", shop.event_id);
    let reversed: Vec<String> = ids.iter().rev().cloned().collect();
    let ordered = app
        .call(
            Method::PUT,
            &order_path,
            Some(&shop.organizer),
            Some(json!({ "image_ids": reversed })),
        )
        .await;
    assert_eq!(ordered.status, StatusCode::OK, "{:?}", ordered.body);
    let listed: Vec<Value> = ordered.body.as_array().unwrap().clone();
    let listed_ids: Vec<&str> = listed
        .iter()
        .map(|image| image["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        listed_ids,
        reversed.iter().map(String::as_str).collect::<Vec<_>>()
    );

    let incomplete = app
        .call(
            Method::PUT,
            &order_path,
            Some(&shop.organizer),
            Some(json!({ "image_ids": [ids[0]] })),
        )
        .await;
    assert_eq!(incomplete.status, StatusCode::UNPROCESSABLE_ENTITY);

    let path = format!("/v1/events/{}/images/{}", shop.event_id, ids[0]);
    let deleted = app
        .call(Method::DELETE, &path, Some(&shop.organizer), None)
        .await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT);
    assert_eq!(
        app.call(Method::GET, &path, None, None).await.status,
        StatusCode::NOT_FOUND
    );
    let key = format!("events/{}/images/{}.png", shop.event_id, ids[0]);
    assert!(
        store.head(&Path::from(key)).await.is_err(),
        "the object is gone"
    );
}

#[tokio::test]
async fn a_public_base_url_serves_images_elsewhere() {
    let app = app_with(Arc::new(InMemory::new()), |config| {
        config.images.public_base_url = Some("https://cdn.example.org/kippu/".to_owned());
    })
    .await;
    let shop = app.shop(10, 1_000, json!({})).await;
    let uploaded = upload(&app, &shop, &shop.organizer, "image/png", PNG).await;
    let image_id = uploaded.body["id"].as_str().unwrap();
    let public = format!(
        "https://cdn.example.org/kippu/events/{}/images/{image_id}.png",
        shop.event_id
    );
    assert_eq!(uploaded.body["url"], public);
    let redirected = app
        .call(
            Method::GET,
            &format!("/v1/events/{}/images/{image_id}", shop.event_id),
            None,
            None,
        )
        .await;
    assert_eq!(redirected.status, StatusCode::FOUND);
    assert_eq!(redirected.headers["location"], public.as_str());
}

#[tokio::test]
async fn without_object_storage_images_are_unavailable() {
    let app = TestApp::start().await;
    let shop = app.shop(10, 1_000, json!({})).await;
    let refused = upload(&app, &shop, &shop.organizer, "image/png", PNG).await;
    assert_eq!(refused.status, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(
        refused.body["type"],
        "urn:kippu:problem:images-not-configured"
    );
}
