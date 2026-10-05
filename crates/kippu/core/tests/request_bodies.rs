//! Request bodies that cannot be read are answered with problems, not axum's plain text.

mod support;

use axum::http::{Method, StatusCode};
use support::{Reply, TestApp};

const REGISTER: &str = "/v1/auth/register";
const JSON: (&str, &str) = ("content-type", "application/json");

fn assert_problem(reply: &Reply, status: StatusCode, kind: &str) {
    assert_eq!(reply.status, status, "{:?}", reply.body);
    assert_eq!(reply.headers["content-type"], "application/problem+json");
    assert_eq!(reply.body["type"], format!("urn:kippu:problem:{kind}"));
    assert_eq!(reply.body["status"], status.as_u16());
}

#[tokio::test]
async fn malformed_json_is_invalid_json() {
    let app = TestApp::start().await;
    let reply = app
        .call_raw(Method::POST, REGISTER, &[JSON], r#"{"email": "#)
        .await;
    assert_problem(&reply, StatusCode::BAD_REQUEST, "invalid-json");
}

#[tokio::test]
async fn json_of_the_wrong_shape_is_invalid_json() {
    let app = TestApp::start().await;
    let reply = app
        .call_raw(Method::POST, REGISTER, &[JSON], r#"{"email": 42}"#)
        .await;
    assert_problem(&reply, StatusCode::UNPROCESSABLE_ENTITY, "invalid-json");
}

#[tokio::test]
async fn a_body_without_json_content_type_is_unsupported() {
    let app = TestApp::start().await;
    let body =
        r#"{"email": "a@example.org", "password": "correct horse battery", "display_name": "A"}"#;
    for headers in [&[][..], &[("content-type", "text/plain")]] {
        let reply = app.call_raw(Method::POST, REGISTER, headers, body).await;
        assert_problem(
            &reply,
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported-media-type",
        );
    }
}

#[tokio::test]
async fn a_body_over_the_limit_is_too_large() {
    let app = TestApp::start().await;
    let limit = app.app.state().config().server.max_body_bytes;
    let body = vec![b' '; limit + 1];
    let declared = body.len().to_string();

    // Declared up front: rejected before the body is read.
    let reply = app
        .call_raw(
            Method::POST,
            REGISTER,
            &[JSON, ("content-length", &declared)],
            body.clone(),
        )
        .await;
    assert_problem(&reply, StatusCode::PAYLOAD_TOO_LARGE, "payload-too-large");

    // Undeclared (as when streamed): the limit trips while the body is read.
    let reply = app.call_raw(Method::POST, REGISTER, &[JSON], body).await;
    assert_problem(&reply, StatusCode::PAYLOAD_TOO_LARGE, "payload-too-large");
}
