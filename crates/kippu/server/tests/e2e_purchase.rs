//! End to end, over real TCP: boot `kippu serve` (with its background workers) from a config
//! file, then walk the whole journey — root bootstraps the hierarchy, an organizer opens a sale
//! behind a waiting room, a buyer queues, buys and pays through an attestor — and finally
//! verify the ticket offline with Kaisatsu, as a gate would.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test: a failure is the test failing"
)]

use std::fmt::Write as _;
use std::net::SocketAddr;
use std::time::Duration as StdDuration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use ed25519_dalek::SigningKey;
use kippu_core::auth::tokens::mint_root_token;
use kippu_core::modules::payments::signature::sign_request;
use kippu_core::{Clock, SystemClock};
use kippu_domain::Duration;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;

const ISSUER: &str = "kippu.e2e";

/// A minimal HTTP/1.1 client: enough to talk JSON to Kippu without extra dependencies.
struct Client {
    address: SocketAddr,
}

struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: Value,
}

impl Reply {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

impl Client {
    async fn send(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Option<&Value>,
    ) -> Reply {
        let body = body.map(Value::to_string).unwrap_or_default();
        let mut request =
            format!("{method} {path} HTTP/1.1\r\nhost: kippu\r\nconnection: close\r\n");
        for (name, value) in headers {
            write!(request, "{name}: {value}\r\n").unwrap();
        }
        if !body.is_empty() {
            request.push_str("content-type: application/json\r\n");
        }
        write!(request, "content-length: {}\r\n\r\n{body}", body.len()).unwrap();

        let mut stream = TcpStream::connect(self.address).await.unwrap();
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).await.unwrap();
        let raw = String::from_utf8(raw).unwrap();
        let (head, body) = raw.split_once("\r\n\r\n").unwrap();
        let mut lines = head.lines();
        let status = lines
            .next()
            .unwrap()
            .split(' ')
            .nth(1)
            .unwrap()
            .parse()
            .unwrap();
        let headers = lines
            .filter_map(|line| line.split_once(": "))
            .map(|(name, value)| (name.to_owned(), value.to_owned()))
            .collect::<Vec<_>>();
        let chunked = headers.iter().any(|(name, value)| {
            name.eq_ignore_ascii_case("transfer-encoding") && value == "chunked"
        });
        let body = if chunked {
            dechunk(body)
        } else {
            body.to_owned()
        };
        Reply {
            status,
            headers,
            body: serde_json::from_str(&body).unwrap_or(Value::String(body)),
        }
    }

    async fn call(
        &self,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> Reply {
        let bearer = token.map(|token| format!("Bearer {token}"));
        let headers: Vec<(&str, &str)> = bearer
            .iter()
            .map(|value| ("authorization", value.as_str()))
            .collect();
        self.send(method, path, &headers, body.as_ref()).await
    }
}

fn dechunk(mut body: &str) -> String {
    let mut out = String::new();
    while let Some((size, rest)) = body.split_once("\r\n") {
        let size = usize::from_str_radix(size.trim(), 16).unwrap_or(0);
        if size == 0 {
            break;
        }
        out.push_str(&rest[..size]);
        body = &rest[size + 2..];
    }
    out
}

async fn poll<F, Fut>(mut attempt: F) -> Value
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Option<Value>>,
{
    for _ in 0..100 {
        if let Some(value) = attempt().await {
            return value;
        }
        tokio::time::sleep(StdDuration::from_millis(50)).await;
    }
    panic!("gave up waiting");
}

fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[expect(clippy::too_many_lines, reason = "one journey, told in order")]
async fn a_ticket_bought_through_the_whole_pipeline_passes_the_gate() {
    // ── Boot from a configuration file, exactly as `kippu serve --config` does ─────────────
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("kippu.toml");
    std::fs::write(
        &config_path,
        format!(
            r#"
            [database]
            url = "sqlite://{database}"

            [issuer]
            id = "{ISSUER}"

            [keys]
            ticket_signing_key = "{ticket_key}"
            token_signing_key = "{token_key}"

            [[root.keys]]
            name = "owner"
            public_key = "{root_public}"

            [workers]
            purchase_interval_ms = 50
            admission_interval_ms = 50
            "#,
            database = dir.path().join("kippu.db").display(),
            ticket_key = STANDARD.encode(key(1).to_bytes()),
            token_key = STANDARD.encode(key(2).to_bytes()),
            root_public = STANDARD.encode(key(3).verifying_key().as_bytes()),
        ),
    )
    .unwrap();
    let figment = kippu_server::config::sources(Some(&config_path));
    let config = kippu_server::config::extract(&figment).unwrap();
    let app = kippu_server::assemble(config, kippu_core::default_modules())
        .await
        .unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = Client {
        address: listener.local_addr().unwrap(),
    };
    let shutdown = CancellationToken::new();
    let server = tokio::spawn({
        let shutdown = shutdown.clone();
        async move { kippu_server::serve_app(&app, listener, shutdown, true).await }
    });
    assert_eq!(client.call("GET", "/readyz", None, None).await.status, 200);

    // ── Root bootstraps an admin; the admin sets up an organization and its organizer ──────
    let now = SystemClock.now();
    let root = mint_root_token(&key(3), "owner", ISSUER, now, Duration::minutes(5));
    let password = "correct horse battery";
    let account = |email: &str, role: &str| json!({ "email": email, "password": password, "display_name": role, "role": role });
    let login = async |email: &str| -> String {
        let session = client
            .call(
                "POST",
                "/v1/auth/login",
                None,
                Some(json!({ "email": email, "password": password })),
            )
            .await;
        session.body["access_token"]["token"]
            .as_str()
            .unwrap()
            .to_owned()
    };

    assert_eq!(
        client
            .call(
                "POST",
                "/v1/admin/accounts",
                Some(&root),
                Some(account("admin@e2e.test", "admin"))
            )
            .await
            .status,
        201
    );
    let admin = login("admin@e2e.test").await;
    let organization = client
        .call(
            "POST",
            "/v1/admin/organizations",
            Some(&admin),
            Some(json!({ "slug": "comiket", "name": "Comiket" })),
        )
        .await;
    let organization_id = organization.body["id"].as_str().unwrap().to_owned();
    let organizer = client
        .call(
            "POST",
            "/v1/admin/accounts",
            Some(&admin),
            Some(account("staff@e2e.test", "organizer")),
        )
        .await;
    let organizer_id = organizer.body["id"].as_str().unwrap();
    let membership = client
        .call(
            "PUT",
            &format!("/v1/admin/organizations/{organization_id}/members/{organizer_id}"),
            Some(&admin),
            None,
        )
        .await;
    assert_eq!(membership.status, 204);
    let organizer = login("staff@e2e.test").await;

    // ── The organizer opens a sale behind a waiting room ───────────────────────────────────
    let event = client
        .call(
            "POST",
            &format!("/v1/organizations/{organization_id}/events"),
            Some(&organizer),
            Some(json!({
                "slug": "c107", "title": "Comic Market 107", "venue": "Tokyo Big Sight",
                "starts_at": (now + Duration::days(60)).to_string(),
                "ends_at": (now + Duration::days(62)).to_string(),
            })),
        )
        .await;
    let event_id = event.body["id"].as_str().unwrap().to_owned();
    let sale = client
        .call(
            "POST",
            &format!("/v1/events/{event_id}/sales"),
            Some(&organizer),
            Some(json!({
                "name": "General",
                "opens_at": (now - Duration::minutes(1)).to_string(),
                "closes_at": (now + Duration::days(7)).to_string(),
                "admission": { "mode": "waiting_room", "admit_per_tick": 10, "max_backlog": 100 },
            })),
        )
        .await;
    assert_eq!(sale.status, 201, "{:?}", sale.body);
    let sale_id = sale.body["id"].as_str().unwrap().to_owned();
    let ticket_type = client
        .call(
            "POST",
            &format!("/v1/sales/{sale_id}/ticket-types"),
            Some(&organizer),
            Some(json!({
                "name": "Day 1",
                "price": { "amount_minor": 2500, "currency": "JPY" },
                "capacity": 5,
                "valid_from": (now + Duration::days(60)).to_string(),
                "valid_until": (now + Duration::days(61)).to_string(),
                "ticket_extensions": { "128": "EAST-1" },
            })),
        )
        .await;
    let ticket_type_id = ticket_type.body["id"].as_str().unwrap().to_owned();

    // The admin registers a payment attestor; the organizer accepts it for this sale.
    let attestor_key = key(4);
    let attestor = client
        .call(
            "POST",
            "/v1/admin/attestors",
            Some(&admin),
            Some(json!({
                "name": "Card gateway", "environment": "live",
                "keys": [{ "key_id": "k1", "public_key": STANDARD.encode(attestor_key.verifying_key().as_bytes()) }],
            })),
        )
        .await;
    let attestor_id = attestor.body["id"].as_str().unwrap().to_owned();
    let mut sale = sale.body;
    sale["accepted_attestors"] = json!([attestor_id]);
    assert_eq!(
        client
            .call(
                "PUT",
                &format!("/v1/sales/{sale_id}"),
                Some(&organizer),
                Some(sale)
            )
            .await
            .status,
        200
    );
    let mut event = event.body;
    event["status"] = json!("published");
    assert_eq!(
        client
            .call(
                "PUT",
                &format!("/v1/events/{event_id}"),
                Some(&organizer),
                Some(event)
            )
            .await
            .status,
        200
    );

    // ── A buyer queues, is admitted, and submits a purchase ────────────────────────────────
    let registered = client
        .call(
            "POST",
            "/v1/auth/register",
            None,
            Some(json!({ "email": "miku@e2e.test", "password": password, "display_name": "Miku" })),
        )
        .await;
    assert_eq!(registered.status, 201);
    let buyer = login("miku@e2e.test").await;
    let place = client
        .call(
            "POST",
            &format!("/v1/sales/{sale_id}/waiting-room"),
            Some(&buyer),
            None,
        )
        .await;
    assert_eq!(place.body["position"], 1);
    let queue_ticket = place.body["queue_ticket"]["token"].clone();
    let admission = poll(|| async {
        let reply = client
            .call(
                "POST",
                &format!("/v1/sales/{sale_id}/admission"),
                Some(&buyer),
                Some(json!({ "queue_ticket": queue_ticket })),
            )
            .await;
        (reply.body["status"] == "admitted").then_some(reply.body)
    })
    .await;
    let pass = admission["admission_pass"]["token"].as_str().unwrap();

    let bearer = format!("Bearer {buyer}");
    let basket = json!({ "items": [{ "ticket_type_id": ticket_type_id, "quantity": 2 }] });
    let submitted = client
        .send(
            "POST",
            &format!("/v1/sales/{sale_id}/purchase-requests"),
            &[
                ("authorization", &bearer),
                ("idempotency-key", "e2e-1"),
                ("kippu-admission-pass", pass),
            ],
            Some(&basket),
        )
        .await;
    assert_eq!(submitted.status, 202, "{:?}", submitted.body);
    let location = submitted.header("location").unwrap().to_owned();

    // The background worker (running inside `serve`) reserves the tickets.
    let request = poll(|| async {
        let reply = client.call("GET", &location, Some(&buyer), None).await;
        (reply.body["status"] != "queued").then_some(reply.body)
    })
    .await;
    assert_eq!(request["status"], "reserved", "{request:?}");
    let reservation_id = request["reservation_id"].as_str().unwrap().to_owned();

    // ── Checkout, and the attestor reports the payment ─────────────────────────────────────
    let checkout = client
        .call(
            "POST",
            &format!("/v1/reservations/{reservation_id}/checkout"),
            Some(&buyer),
            Some(json!({ "attestor_id": attestor_id })),
        )
        .await;
    assert_eq!(checkout.body["status"], "payment_pending");
    let signed = |method: &str, path: &str, body: &str| {
        sign_request(
            &attestor_key,
            attestor_id.parse().unwrap(),
            "k1",
            method,
            path,
            SystemClock.now().unix_seconds(),
            body.as_bytes(),
        )
    };
    let feed_signature = signed("GET", "/v1/attestor/feed", "");
    let feed = client
        .send(
            "GET",
            "/v1/attestor/feed",
            &[("kippu-signature", &feed_signature)],
            None,
        )
        .await;
    let requested = &feed.body[0]["event"];
    assert_eq!(requested["topic"], "payment.requested");
    assert_eq!(
        requested["amount"],
        json!({ "amount_minor": 5000, "currency": "JPY" })
    );

    let attestation = json!({
        "attestation_id": "ch_e2e_1", "outcome": "paid",
        "reservation_id": reservation_id, "amount": requested["amount"],
    });
    let signature = signed("POST", "/v1/payment-attestations", &attestation.to_string());
    let paid = client
        .send(
            "POST",
            "/v1/payment-attestations",
            &[("kippu-signature", &signature)],
            Some(&attestation),
        )
        .await;
    assert_eq!(paid.status, 200, "{:?}", paid.body);
    assert_eq!(paid.body["disposition"], "applied");
    assert_eq!(paid.body["reservation"]["status"], "issued");

    // ── At the gate: verify offline with Kaisatsu and the published keys ──────────────────
    let tickets = client
        .call("GET", "/v1/me/tickets", Some(&buyer), None)
        .await;
    let tickets = tickets.body["items"].as_array().unwrap().clone();
    assert_eq!(tickets.len(), 2);
    let keys = client
        .call("GET", "/.well-known/kippu/ticket-keys", None, None)
        .await;
    assert_eq!(keys.body["issuer"], ISSUER);
    let trusted: Vec<kaisatsu::TrustedKey> = keys.body["keys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|key| {
            let bytes: [u8; 32] = STANDARD
                .decode(key["public_key"].as_str().unwrap())
                .unwrap()
                .try_into()
                .unwrap();
            kaisatsu::TrustedKey::from_bytes(&bytes).unwrap()
        })
        .collect();
    let gate = kaisatsu::Verifier::new(trusted.as_slice());

    let bytes = STANDARD
        .decode(tickets[0]["ticket"].as_str().unwrap())
        .unwrap();
    let ticket = gate
        .verify(&bytes)
        .expect("an issued ticket passes the gate");
    assert_eq!(ticket.issuer(), ISSUER);
    assert_eq!(ticket.event_id().to_string(), event_id);
    assert_eq!(ticket.ticket_type_id().to_string(), ticket_type_id);
    assert_eq!(ticket.extensions().get(0x80), Some(&b"EAST-1"[..]));
    let opening = (now + Duration::days(60))
        .unix_seconds()
        .try_into()
        .unwrap();
    assert_eq!(ticket.check_time(opening), Ok(()));
    assert_eq!(
        ticket.check_time(opening - 1),
        Err(kaisatsu::Pinpon::NotYetValid)
    );

    // A forged ticket: one byte changed.
    let mut forged = bytes.clone();
    forged[20] ^= 0x01;
    let pinpon = gate.verify(&forged).unwrap_err();
    assert_eq!(pinpon, kaisatsu::Pinpon::BadSignature);
    assert_eq!(pinpon.to_string(), "ピンポーン🔔 BadSignature");

    // ── Graceful shutdown ──────────────────────────────────────────────────────────────────
    shutdown.cancel();
    server.await.unwrap().unwrap();
}
