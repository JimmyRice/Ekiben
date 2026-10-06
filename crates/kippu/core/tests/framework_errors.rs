//! Errors the framework answers — unknown routes, wrong methods, unparsable parameters — are
//! problems like every other error.

mod support;

use axum::http::{Method, StatusCode};
use support::{Reply, TestApp};

fn assert_problem(reply: &Reply, status: StatusCode, kind: &str) {
    assert_eq!(reply.status, status, "{:?}", reply.body);
    assert_eq!(reply.headers["content-type"], "application/problem+json");
    assert_eq!(reply.body["type"], format!("urn:kippu:problem:{kind}"));
    assert_eq!(reply.body["status"], status.as_u16());
}

#[tokio::test]
async fn an_unknown_route_is_not_found() {
    let app = TestApp::start().await;
    let reply = app.call_raw(Method::GET, "/v1/nothing-here", &[], "").await;
    assert_problem(&reply, StatusCode::NOT_FOUND, "not-found");
    assert_eq!(reply.body["detail"], "no route matches this path");
}

#[tokio::test]
async fn a_wrong_method_keeps_its_allow_header() {
    let app = TestApp::start().await;
    let reply = app.call_raw(Method::DELETE, "/healthz", &[], "").await;
    assert_problem(&reply, StatusCode::METHOD_NOT_ALLOWED, "method-not-allowed");
    assert!(reply.headers.contains_key("allow"), "{:?}", reply.headers);
}

#[tokio::test]
async fn an_unparsable_path_parameter_names_the_parameter() {
    let app = TestApp::start().await;
    let reply = app.call_raw(Method::GET, "/v1/events/nope", &[], "").await;
    assert_problem(&reply, StatusCode::BAD_REQUEST, "invalid-parameter");
    assert!(
        reply.body["detail"].as_str().unwrap().contains("event_id"),
        "{:?}",
        reply.body
    );
}
