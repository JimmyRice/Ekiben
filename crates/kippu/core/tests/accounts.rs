//! Accounts, roles, sessions and idempotency through the HTTP API.

mod support;

use axum::http::{Method, StatusCode};
use kippu_domain::Duration;
use serde_json::json;
use support::TestApp;

#[tokio::test]
async fn root_signs_in_with_a_key_and_bootstraps_the_hierarchy() {
    let app = TestApp::start().await;
    let root = app.root_token();

    let me = app.call(Method::GET, "/v1/me", Some(&root), None).await;
    assert_eq!(me.status, StatusCode::OK);
    assert_eq!(me.body, json!({ "kind": "root", "key_name": "owner" }));

    let (admin, _) = app.account("admin", "admin@example.org").await;
    let organization = app
        .call(
            Method::POST,
            "/v1/admin/organizations",
            Some(&admin),
            Some(json!({ "slug": "circle", "name": "Circle" })),
        )
        .await;
    assert_eq!(organization.status, StatusCode::CREATED);
    let organization_id = organization.body["id"].as_str().unwrap();

    let created = app
        .call(
            Method::POST,
            "/v1/admin/accounts",
            Some(&admin),
            Some(json!({ "email": "staff@example.org", "password": "correct horse battery", "display_name": "Staff", "role": "organizer" })),
        )
        .await;
    assert_eq!(created.status, StatusCode::CREATED);
    let organizer_id = created.body["id"].as_str().unwrap();
    let added = app
        .call(
            Method::PUT,
            &format!("/v1/admin/organizations/{organization_id}/members/{organizer_id}"),
            Some(&admin),
            None,
        )
        .await;
    assert_eq!(added.status, StatusCode::NO_CONTENT);

    let (organizer, _) = app.login("staff@example.org").await;
    let me = app
        .call(Method::GET, "/v1/me", Some(&organizer), None)
        .await;
    assert_eq!(me.body["kind"], "account");
    assert_eq!(me.body["organizations"], json!([organization_id]));

    let audit = app
        .call(Method::GET, "/v1/admin/audit-log", Some(&root), None)
        .await;
    let actions: Vec<_> = audit.body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["action"].clone())
        .collect();
    assert!(actions.contains(&json!("organization.member.add")));
}

#[tokio::test]
async fn roles_only_manage_strictly_lower_roles() {
    let app = TestApp::start().await;
    let (admin, _) = app.account("admin", "admin@example.org").await;
    let (organizer, _) = app.account("organizer", "organizer@example.org").await;
    let (_, other_admin) = app.account("admin", "other-admin@example.org").await;

    let create = |role: &str, email: &str| json!({ "email": email, "password": "correct horse battery", "display_name": "x", "role": role });
    let peer = app
        .call(
            Method::POST,
            "/v1/admin/accounts",
            Some(&admin),
            Some(create("admin", "peer@example.org")),
        )
        .await;
    assert_eq!(
        peer.status,
        StatusCode::FORBIDDEN,
        "admins cannot create admins"
    );
    let root = app
        .call(
            Method::POST,
            "/v1/admin/accounts",
            Some(&app.root_token()),
            Some(create("root", "r@example.org")),
        )
        .await;
    assert_eq!(
        root.status,
        StatusCode::FORBIDDEN,
        "root is never an account"
    );
    let by_organizer = app
        .call(
            Method::POST,
            "/v1/admin/accounts",
            Some(&organizer),
            Some(create("user", "u@example.org")),
        )
        .await;
    assert_eq!(
        by_organizer.status,
        StatusCode::FORBIDDEN,
        "organizers manage events, not people"
    );

    let id = other_admin["id"].as_str().unwrap();
    let deleted = app
        .call(
            Method::DELETE,
            &format!("/v1/admin/accounts/{id}"),
            Some(&admin),
            None,
        )
        .await;
    assert_eq!(deleted.status, StatusCode::FORBIDDEN);
    let deleted = app
        .call(
            Method::DELETE,
            &format!("/v1/admin/accounts/{id}"),
            Some(&app.root_token()),
            None,
        )
        .await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn users_register_and_refresh_tokens_rotate_once() {
    let app = TestApp::start().await;
    let registered = app
        .call(
            Method::POST,
            "/v1/auth/register",
            None,
            Some(json!({ "email": "Miku@Example.org", "password": "correct horse battery", "display_name": "Miku" })),
        )
        .await;
    assert_eq!(registered.status, StatusCode::CREATED);
    assert_eq!(registered.body["role"], "user");
    assert_eq!(registered.body["email"], "miku@example.org");

    let wrong = app
        .call(
            Method::POST,
            "/v1/auth/login",
            None,
            Some(json!({ "email": "miku@example.org", "password": "nope nope nope" })),
        )
        .await;
    assert_eq!(wrong.status, StatusCode::UNAUTHORIZED);

    let session = app
        .call(
            Method::POST,
            "/v1/auth/login",
            None,
            Some(json!({ "email": "miku@example.org", "password": "correct horse battery" })),
        )
        .await;
    let refresh_token = session.body["refresh_token"].as_str().unwrap();
    let rotated = app
        .call(
            Method::POST,
            "/v1/auth/refresh",
            None,
            Some(json!({ "refresh_token": refresh_token })),
        )
        .await;
    assert_eq!(rotated.status, StatusCode::OK);
    let replayed = app
        .call(
            Method::POST,
            "/v1/auth/refresh",
            None,
            Some(json!({ "refresh_token": refresh_token })),
        )
        .await;
    assert_eq!(
        replayed.status,
        StatusCode::UNAUTHORIZED,
        "a refresh token works once"
    );
}

#[tokio::test]
async fn tokens_expire_and_root_tokens_are_short_lived() {
    let app = TestApp::start().await;
    let (user, _) = app.account("user", "user@example.org").await;
    let root = app.root_token();

    app.clock.advance(Duration::minutes(16));
    assert_eq!(
        app.call(Method::GET, "/v1/me", Some(&user), None)
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.call(Method::GET, "/v1/me", Some(&root), None)
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );

    let forged = app
        .call(Method::GET, "/v1/me", Some("e30.e30.e30"), None)
        .await;
    assert_eq!(forged.status, StatusCode::UNAUTHORIZED);
    assert_eq!(forged.headers["content-type"], "application/problem+json");
}

#[tokio::test]
async fn idempotency_keys_replay_the_first_response() {
    let app = TestApp::start().await;
    let root = app.root_token();
    let body = json!({ "slug": "replayed", "name": "Replayed" });
    let first = app
        .call_with(
            Method::POST,
            "/v1/admin/organizations",
            Some(&root),
            Some(body.clone()),
            &[("idempotency-key", "k-1")],
        )
        .await;
    let second = app
        .call_with(
            Method::POST,
            "/v1/admin/organizations",
            Some(&root),
            Some(body),
            &[("idempotency-key", "k-1")],
        )
        .await;
    assert_eq!(first.status, StatusCode::CREATED);
    assert_eq!(second.status, StatusCode::CREATED);
    assert_eq!(second.body, first.body);
    assert_eq!(second.headers["idempotent-replayed"], "true");

    let different = app
        .call_with(
            Method::POST,
            "/v1/admin/organizations",
            Some(&root),
            Some(json!({ "slug": "other", "name": "Other" })),
            &[("idempotency-key", "k-1")],
        )
        .await;
    assert_eq!(different.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        different.body["type"],
        "urn:kippu:problem:idempotency-key-reused"
    );
}

#[tokio::test]
async fn routes_idempotent_by_design_are_real_routes() {
    let app = TestApp::start().await;
    let openapi = app
        .call(Method::GET, "/openapi.json", None, None)
        .await
        .body;
    for module in kippu_core::default_modules() {
        for route in module.idempotent_routes() {
            let method = route.method.as_str().to_ascii_lowercase();
            assert!(
                openapi["paths"][route.path][&method].is_object(),
                "{} declares {method} {}, which is not a route",
                module.name(),
                route.path
            );
        }
    }
}

#[tokio::test]
async fn health_and_openapi_are_served() {
    let app = TestApp::start().await;
    assert_eq!(
        app.call(Method::GET, "/healthz", None, None).await.status,
        StatusCode::OK
    );
    assert_eq!(
        app.call(Method::GET, "/readyz", None, None).await.status,
        StatusCode::OK
    );
    let openapi = app.call(Method::GET, "/openapi.json", None, None).await;
    assert_eq!(openapi.status, StatusCode::OK);
    assert!(openapi.body["paths"]["/v1/auth/login"]["post"].is_object());

    // Every schema a path or schema refers to is defined, and query parameters are documented
    // as such.
    let text = openapi.body.to_string();
    for reference in text.split("\"#/components/schemas/").skip(1) {
        let name = reference.split('"').next().unwrap();
        assert!(
            openapi.body["components"]["schemas"][name].is_object(),
            "{name} is referred to but not defined"
        );
    }
    let parameters = &openapi.body["paths"]["/v1/events"]["get"]["parameters"];
    assert!(
        parameters
            .as_array()
            .unwrap()
            .iter()
            .all(|parameter| parameter["in"] == "query" && parameter["required"] == false),
        "{parameters}"
    );
}

#[tokio::test]
async fn admins_read_and_rename_organizations() {
    let app = TestApp::start().await;
    let (admin, _) = app.account("admin", "admin@example.org").await;
    let create = |slug: &'static str| {
        app.call(
            Method::POST,
            "/v1/admin/organizations",
            Some(&admin),
            Some(json!({ "slug": slug, "name": "Circle" })),
        )
    };
    let organization = create("circle").await.body;
    create("taken").await;
    let path = format!(
        "/v1/admin/organizations/{}",
        organization["id"].as_str().unwrap()
    );

    let read = app.call(Method::GET, &path, Some(&admin), None).await;
    assert_eq!(read.status, StatusCode::OK);
    assert_eq!(read.body["version"], 1);
    let renamed = app
        .call(
            Method::PATCH,
            &path,
            Some(&admin),
            Some(json!({ "version": 1, "name": "Circle Club" })),
        )
        .await;
    assert_eq!(renamed.status, StatusCode::OK, "{:?}", renamed.body);
    assert_eq!(
        (
            &renamed.body["name"],
            &renamed.body["slug"],
            &renamed.body["version"]
        ),
        (&json!("Circle Club"), &json!("circle"), &json!(2))
    );
    for (body, status) in [
        (
            json!({ "version": 1, "slug": "circle", "name": "Stale" }),
            StatusCode::PRECONDITION_FAILED,
        ),
        (
            json!({ "version": 2, "slug": "taken", "name": "Taken" }),
            StatusCode::CONFLICT,
        ),
        (
            json!({ "version": 2, "slug": "circle", "name": " " }),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            json!({ "version": 2, "slug": "Not A Slug", "name": "Circle" }),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
    ] {
        let refused = app.call(Method::PUT, &path, Some(&admin), Some(body)).await;
        assert_eq!(refused.status, status, "{:?}", refused.body);
    }
    let replaced = app
        .call(
            Method::PUT,
            &path,
            Some(&admin),
            Some(json!({ "version": 2, "slug": "circle-club", "name": "Circle Club" })),
        )
        .await;
    assert_eq!(replaced.body["slug"], "circle-club", "{:?}", replaced.body);
    let audit = app
        .call(
            Method::GET,
            "/v1/admin/audit-log?limit=1",
            Some(&admin),
            None,
        )
        .await;
    assert_eq!(audit.body["items"][0]["action"], "organization.update");
}

#[tokio::test]
async fn admins_list_members_and_the_organizations_of_an_account() {
    let app = TestApp::start().await;
    let (admin, _) = app.account("admin", "admin@example.org").await;
    let organization = app
        .call(
            Method::POST,
            "/v1/admin/organizations",
            Some(&admin),
            Some(json!({ "slug": "circle", "name": "Circle" })),
        )
        .await
        .body;
    let path = format!(
        "/v1/admin/organizations/{}",
        organization["id"].as_str().unwrap()
    );

    // Members, in account id order, a page at a time.
    let mut members = Vec::new();
    for i in 0..3 {
        let (_, account) = app
            .account("organizer", &format!("organizer{i}@example.org"))
            .await;
        let id = account["id"].as_str().unwrap().to_owned();
        if i != 1 {
            let added = app
                .call(
                    Method::PUT,
                    &format!("{path}/members/{id}"),
                    Some(&admin),
                    None,
                )
                .await;
            assert_eq!(added.status, StatusCode::NO_CONTENT);
            members.push(id);
        }
    }
    members.sort();
    let first = app
        .call(
            Method::GET,
            &format!("{path}/members?limit=1"),
            Some(&admin),
            None,
        )
        .await;
    assert_eq!(first.status, StatusCode::OK, "{:?}", first.body);
    assert_eq!(first.body["items"][0]["id"], json!(members[0]));
    let cursor = first.body["next_cursor"].as_str().unwrap();
    let second = app
        .call(
            Method::GET,
            &format!("{path}/members?limit=1&cursor={cursor}"),
            Some(&admin),
            None,
        )
        .await;
    assert_eq!(second.body["items"][0]["id"], json!(members[1]));

    // An account shows the organizations it belongs to.
    let account = app
        .call(
            Method::GET,
            &format!("/v1/admin/accounts/{}", members[0]),
            Some(&admin),
            None,
        )
        .await;
    assert_eq!(account.status, StatusCode::OK);
    assert_eq!(account.body["account"]["id"], json!(members[0]));
    assert_eq!(account.body["organizations"], json!([organization["id"]]));

    // Organizers cannot use the administration, and unknown records are missing.
    let (organizer, _) = app.login("organizer0@example.org").await;
    for path in [
        path.clone(),
        format!("{path}/members"),
        format!("/v1/admin/accounts/{}", members[0]),
    ] {
        let forbidden = app.call(Method::GET, &path, Some(&organizer), None).await;
        assert_eq!(forbidden.status, StatusCode::FORBIDDEN, "{path}");
    }
    let unknown = "0190a7c4-0000-7000-8000-000000000000";
    for path in [
        format!("/v1/admin/organizations/{unknown}"),
        format!("/v1/admin/organizations/{unknown}/members"),
        format!("/v1/admin/accounts/{unknown}"),
    ] {
        let missing = app.call(Method::GET, &path, Some(&admin), None).await;
        assert_eq!(missing.status, StatusCode::NOT_FOUND, "{path}");
    }
}

#[tokio::test]
async fn admins_find_accounts_by_role_and_email() {
    let app = TestApp::start().await;
    let (admin, _) = app.account("admin", "admin@example.org").await;
    for i in 0..3 {
        app.account("organizer", &format!("organizer{i}@example.org"))
            .await;
    }
    app.account("user", "user@example.org").await;

    let organizers = app
        .call(
            Method::GET,
            "/v1/admin/accounts?role=organizer&limit=2",
            Some(&admin),
            None,
        )
        .await;
    assert_eq!(organizers.status, StatusCode::OK, "{:?}", organizers.body);
    assert_eq!(organizers.body["items"].as_array().unwrap().len(), 2);
    let cursor = organizers.body["next_cursor"].as_str().unwrap();
    let rest = app
        .call(
            Method::GET,
            &format!("/v1/admin/accounts?role=organizer&limit=2&cursor={cursor}"),
            Some(&admin),
            None,
        )
        .await;
    assert_eq!(rest.body["items"].as_array().unwrap().len(), 1);
    assert_eq!(rest.body["next_cursor"], json!(null));
    for item in [&organizers.body["items"][0], &rest.body["items"][0]] {
        assert_eq!(item["role"], "organizer");
    }

    let found = app
        .call(
            Method::GET,
            "/v1/admin/accounts?email=organizer1@example.org",
            Some(&admin),
            None,
        )
        .await;
    assert_eq!(found.body["items"][0]["email"], "organizer1@example.org");
    assert_eq!(found.body["items"].as_array().unwrap().len(), 1);
    let none = app
        .call(
            Method::GET,
            "/v1/admin/accounts?email=organizer1@example.org&role=user",
            Some(&admin),
            None,
        )
        .await;
    assert_eq!(none.body["items"], json!([]));

    for path in [
        "/v1/admin/accounts?email=not-an-email",
        "/v1/admin/accounts?role=owner",
        "/v1/admin/accounts?rol=user",
    ] {
        let refused = app.call(Method::GET, path, Some(&admin), None).await;
        assert_eq!(refused.status, StatusCode::BAD_REQUEST, "{path}");
    }
}
