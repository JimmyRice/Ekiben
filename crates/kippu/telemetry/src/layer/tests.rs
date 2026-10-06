use std::sync::{Arc, Mutex};

use serde_json::Value;
use tracing::field::Empty;
use tracing_subscriber::layer::SubscriberExt as _;

use super::*;
use crate::{REQUEST_SPAN, TASK_SPAN};

#[derive(Clone, Default)]
struct Buffer(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Buffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'writer> tracing_subscriber::fmt::MakeWriter<'writer> for Buffer {
    type Writer = Self;

    fn make_writer(&'writer self) -> Self {
        self.clone()
    }
}

fn run(format: Format, body: impl FnOnce()) -> String {
    let buffer = Buffer::default();
    let subscriber = tracing_subscriber::registry().with(LogLayer::new(format, buffer.clone()));
    tracing::subscriber::with_default(subscriber, body);
    let bytes = buffer.0.lock().unwrap().clone();
    String::from_utf8(bytes).unwrap()
}

fn query(summary: &str) {
    tracing::debug!(target: "sqlx::query", summary, elapsed_secs = 0.0004_f64, "");
}

/// Logs a request the way `kippu-core` does, plus a stray event and an idle task run.
fn capture(format: Format) -> String {
    run(format, || {
        tracing::info!(address = "0.0.0.0:8080", workers = true, "kippu is serving");
        let request = tracing::info_span!(
            REQUEST_SPAN,
            method = "POST",
            path = "/v1/sales/0199/purchase-requests",
            request_id = "5cdc82aa-1111-2222-3333-444455556666",
            client = Empty,
            idempotency_key = "my-first-order",
            caller = Empty,
            problem = Empty,
            origin = Empty,
            status = Empty,
            latency_us = Empty,
        );
        let other = tracing::info_span!(REQUEST_SPAN, method = "GET", path = "/healthz");
        request.in_scope(|| {
            tracing::Span::current().record("caller", "account:0199 (user)");
            other.in_scope(|| tracing::info!("interleaved"));
            tracing::info!(id = "6ba1", "purchase request queued");
            tracing::warn!("evil\nINFO forged line");
            tracing::info_span!("submit_purchase").in_scope(|| {
                query("SELECT 1");
                tracing::info_span!("visible_sale").in_scope(|| {
                    query("SELECT 2");
                    tracing::warn!("sale is not open");
                });
            });
            tracing::Span::current().record("origin", "visible_sale @ sales.rs:58");
        });
        request.record("status", 409_u16);
        request.record("latency_us", 4_800_u64);
        drop(other);
        drop(request);
        let task = tracing::info_span!(TASK_SPAN, task = "expiry", outcome = Empty);
        task.record("outcome", "idle");
        drop(task);
    })
}

#[test]
fn pretty_writes_a_block_per_request() {
    let output = capture(Format::Pretty);
    let blocks: Vec<&str> = output.split("\n\n").collect();
    assert!(
        blocks[0].contains("kippu is serving  address=0.0.0.0:8080 workers=true"),
        "{output}"
    );
    // The inner request closed first, and nothing of it leaked into the outer block.
    assert!(blocks[0].contains("GET /healthz"), "{output}");
    assert!(blocks[0].contains("interleaved"), "{output}");
    let block = blocks[1];
    assert!(block.starts_with("┌─ "), "{output}");
    assert!(
        block.contains("POST /v1/sales/0199/purchase-requests"),
        "{block}"
    );
    assert!(block.contains("req 5cdc82aa"), "{block}");
    assert!(
        block.contains("│  caller            account:0199 (user)"),
        "{block}"
    );
    assert!(
        block.contains("│  idempotency-key   my-first-order"),
        "{block}"
    );
    assert!(
        block.contains("purchase request queued  id=6ba1"),
        "{block}"
    );
    assert!(block.contains("WARN evil\\nINFO forged line"), "{block}");
    assert!(!block.contains("interleaved"), "{block}");
    let foot = block.lines().last().unwrap();
    assert!(foot.starts_with("└─ 409 Conflict"), "{foot}");
    assert!(foot.ends_with("4.8 ms"), "{foot}");
    assert_eq!(foot.chars().count(), WIDTH_OF_BLOCK, "{foot}");
    assert!(
        !output.contains("expiry"),
        "idle task runs are not shown: {output}"
    );
    assert!(output.ends_with("\n\n"));
}

const WIDTH_OF_BLOCK: usize = 80;

#[test]
fn pretty_shows_the_call_chain_and_where_it_failed() {
    let output = capture(Format::Pretty);
    let block = output.split("\n\n").nth(1).unwrap();
    // Nested by call; the step that raised the error and the one it was called from fail.
    assert!(block.contains("✗ layer::tests::submit_purchase"), "{block}");
    assert!(block.contains("  ✗ layer::tests::visible_sale"), "{block}");
    assert!(block.contains("(1 query, 400 µs)"), "{block}");
    assert!(
        block.contains("│  db                2 queries, 800 µs"),
        "{block}"
    );
    assert!(block.contains("    sale is not open"), "{block}");
    assert!(
        block.contains("origin            visible_sale @ sales.rs:58"),
        "{block}"
    );
    assert!(
        !block.contains("SELECT"),
        "statements are counted, not listed: {block}"
    );
}

#[test]
fn pretty_names_a_statement_that_repeats() {
    let output = run(Format::Pretty, || {
        tracing::info_span!(REQUEST_SPAN, method = "GET", path = "/v1/events").in_scope(|| {
            for _ in 0..6 {
                query("SELECT * FROM ticket_types WHERE event_id = ?");
            }
            query("SELECT * FROM events");
        });
    });
    assert!(
        output.contains("7 queries, 2.8 ms · 6× SELECT * FROM ticket_types WHERE event_id = ?"),
        "{output}"
    );
}

#[test]
fn pretty_shows_calls_to_other_systems() {
    let output = run(Format::Pretty, || {
        let request = tracing::info_span!(REQUEST_SPAN, method = "POST", path = "/x");
        request.in_scope(|| {
            let ok = tracing::info_span!(
                crate::DEPENDENCY_SPAN,
                system = "nats",
                operation = "enqueue",
                error = Empty
            );
            drop(ok.enter());
            drop(ok);
            let bad = tracing::info_span!(
                crate::DEPENDENCY_SPAN,
                system = "s3",
                operation = "put",
                error = Empty
            );
            let _entered = bad.enter();
            bad.record("error", "connection refused");
        });
    });
    assert!(output.contains("◆ nats enqueue"), "{output}");
    assert!(output.contains("✗ s3 put  connection refused"), "{output}");
}

#[test]
fn compact_writes_a_line_per_request() {
    let output = capture(Format::Compact);
    let lines: Vec<&str> = output.lines().collect();
    assert_eq!(lines.len(), 3, "{output}");
    let line = lines[2];
    assert!(
        line.contains("409 POST /v1/sales/0199/purchase-requests 4.8 ms req=5cdc82aa"),
        "{line}"
    );
    assert!(line.contains("caller=\"account:0199 (user)\""), "{line}");
    assert!(line.contains("db=2/800µs"), "{line}");
    assert!(
        line.contains("trace=layer::tests::submit_purchase>layer::tests::visible_sale"),
        "{line}"
    );
    assert!(line.contains("| evil\\nINFO forged line"), "{line}");
    assert!(!line.contains("purchase request queued"), "{line}");
}

#[test]
fn json_writes_an_object_per_event() {
    let output = capture(Format::Json);
    let lines: Vec<Value> = output
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(lines[0]["workers"], true, "values keep their type");
    let queued = lines
        .iter()
        .find(|line| line["message"] == "purchase request queued")
        .unwrap();
    assert_eq!(queued["request_id"], "5cdc82aa-1111-2222-3333-444455556666");
    assert_eq!(queued["id"], "6ba1");
    let closed = lines
        .iter()
        .find(|line| line["message"] == "sale is not open")
        .unwrap();
    assert_eq!(closed["step"], "layer::tests::visible_sale");
    assert!(closed["at"].as_str().unwrap().contains("tests.rs:"));
    let finished = lines
        .iter()
        .find(|line| line["message"] == "request finished" && line["method"] == "POST")
        .unwrap();
    assert_eq!(finished["status"], 409);
    assert_eq!(finished["latency_us"], 4_800);
    assert_eq!(finished["caller"], "account:0199 (user)");
    assert_eq!(finished["origin"], "visible_sale @ sales.rs:58");
    assert_eq!(finished["db_queries"], 2);
    assert!(!output.contains("expiry"));
}
