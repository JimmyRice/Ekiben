//! Logging: one unit of output per request, whatever the format.
//!
//! `kippu-core` runs every request in a `request` span and every background task run in a
//! `task` span (see [`kippu_core::http::trace`]). [`RequestLog`] collects the events of each
//! such span and writes them out together when it closes, so concurrent requests never
//! interleave:
//!
//! - `pretty`: a block per request, from arrival to response, with a blank line after it.
//!   Task runs get a small block too, unless they found nothing to do.
//! - `compact`: one line per request.
//! - `json`: one object per line and per event, tagged with `request_id` (or `task`), plus a
//!   `request finished` line per request carrying its status and latency.
//!
//! Pretty and compact output show a request once it is finished: a stuck request shows up when
//! the request timeout ends it with 408.

use std::borrow::Cow;
use std::fmt::Write as _;
use std::io::{IsTerminal, Write as _};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::http::StatusCode;
use kippu_core::http::trace::{REQUEST_SPAN, TASK_SPAN};
use kippu_domain::Timestamp;
use serde_json::Value;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt;

use crate::cli::LogFormat;

/// Installs the global logger. `RUST_LOG` filters as usual (default `info`).
pub(crate) fn init(format: LogFormat) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let colors = format == LogFormat::Pretty
        && std::io::stdout().is_terminal()
        && std::env::var_os("NO_COLOR").is_none_or(|value| value.is_empty());
    let layer = RequestLog::new(format, std::io::stdout).colors(colors);
    // Already installed (e.g. by a test harness): keep the existing logger.
    drop(
        tracing_subscriber::registry()
            .with(filter)
            .with(layer)
            .try_init(),
    );
}

/// A [`tracing_subscriber::Layer`] that writes each request and task run as one unit.
pub(crate) struct RequestLog<W> {
    format: LogFormat,
    writer: W,
    colors: bool,
}

impl<W> RequestLog<W> {
    pub(crate) const fn new(format: LogFormat, writer: W) -> Self {
        Self {
            format,
            writer,
            colors: false,
        }
    }

    /// Uses ANSI colors in `pretty` output.
    pub(crate) const fn colors(mut self, colors: bool) -> Self {
        self.colors = colors;
        self
    }
}

/// What a `request` or `task` span has collected so far.
struct Unit {
    kind: UnitKind,
    started: SystemTime,
    fields: Fields,
    events: Vec<Entry>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum UnitKind {
    Request,
    Task,
}

/// One logged event.
struct Entry {
    at: SystemTime,
    level: Level,
    target: String,
    message: String,
    fields: Fields,
}

/// Field values in recording order; recording a field again replaces its value. Values keep
/// their type (string, number, boolean) for JSON output.
#[derive(Default)]
struct Fields(Vec<(&'static str, Value)>);

impl Fields {
    /// A value as text.
    fn get(&self, name: &str) -> Option<Cow<'_, str>> {
        self.value(name).map(text)
    }

    fn number(&self, name: &str) -> Option<u64> {
        self.value(name).and_then(Value::as_u64)
    }

    fn value(&self, name: &str) -> Option<&Value> {
        self.0
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value)
    }

    fn set(&mut self, name: &'static str, value: Value) {
        match self.0.iter_mut().find(|(key, _)| *key == name) {
            Some((_, slot)) => *slot = value,
            None => self.0.push((name, value)),
        }
    }
}

/// Collects field values; `message` is kept apart.
#[derive(Default)]
struct Collector {
    message: Option<String>,
    fields: Fields,
}

impl Visit for Collector {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.record(field, value.into());
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.record(field, value.into());
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.record(field, value.into());
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.record(field, value.into());
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        self.record(field, value.into());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.record(field, format!("{value:?}").into());
    }
}

impl Collector {
    fn record(&mut self, field: &Field, value: Value) {
        if field.name() == "message" {
            self.message = Some(text(&value).into_owned());
        } else {
            self.fields.set(field.name(), value);
        }
    }
}

/// A field value as text: strings as they are, anything else as JSON.
fn text(value: &Value) -> Cow<'_, str> {
    match value {
        Value::String(text) => Cow::Borrowed(text),
        other => Cow::Owned(other.to_string()),
    }
}

impl<S, W> tracing_subscriber::Layer<S> for RequestLog<W>
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
    W: for<'writer> MakeWriter<'writer> + 'static,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let kind = match attrs.metadata().name() {
            REQUEST_SPAN => UnitKind::Request,
            TASK_SPAN => UnitKind::Task,
            _ => return,
        };
        let Some(span) = ctx.span(id) else { return };
        let mut collector = Collector::default();
        attrs.record(&mut collector);
        span.extensions_mut().insert(Unit {
            kind,
            started: SystemTime::now(),
            fields: collector.fields,
            events: Vec::new(),
        });
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else { return };
        let mut extensions = span.extensions_mut();
        if let Some(unit) = extensions.get_mut::<Unit>() {
            let mut collector = Collector {
                fields: std::mem::take(&mut unit.fields),
                ..Collector::default()
            };
            values.record(&mut collector);
            unit.fields = collector.fields;
        }
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let mut collector = Collector::default();
        event.record(&mut collector);
        let entry = Entry {
            at: SystemTime::now(),
            level: *event.metadata().level(),
            target: event.metadata().target().to_owned(),
            message: collector.message.unwrap_or_default(),
            fields: collector.fields,
        };
        let unit = ctx.event_scope(event).and_then(|scope| {
            scope
                .into_iter()
                .find(|span| span.extensions().get::<Unit>().is_some())
        });
        match (self.format, unit) {
            (LogFormat::Json, unit) => {
                let tag = unit.as_ref().and_then(|span| {
                    let extensions = span.extensions();
                    let unit = extensions.get::<Unit>()?;
                    Some(match unit.kind {
                        UnitKind::Request => {
                            ("request_id", unit.fields.get("request_id")?.into_owned())
                        }
                        UnitKind::Task => ("task", unit.fields.get("task")?.into_owned()),
                    })
                });
                self.write(&json_event(&entry, tag));
            }
            (_, Some(span)) => {
                if let Some(unit) = span.extensions_mut().get_mut::<Unit>() {
                    unit.events.push(entry);
                }
            }
            (LogFormat::Pretty | LogFormat::Compact, None) => {
                self.write(&self.line(&entry));
            }
        }
    }

    fn on_close(&self, id: Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(&id) else { return };
        let Some(unit) = span.extensions_mut().remove::<Unit>() else {
            return;
        };
        let idle = unit.kind == UnitKind::Task
            && unit.events.is_empty()
            && unit.fields.get("outcome").as_deref() != Some("failed");
        if idle {
            return;
        }
        let output = match (self.format, unit.kind) {
            (LogFormat::Pretty, _) => self.block(&unit),
            (LogFormat::Compact, _) => Self::summary(&unit),
            (LogFormat::Json, UnitKind::Request) => json_request(&unit),
            (LogFormat::Json, UnitKind::Task) => return,
        };
        self.write(&output);
    }
}

/// Width of a pretty block, not counting text that does not fit.
const WIDTH: usize = 80;
/// Width of the label column inside a block.
const LABEL: usize = 18;

impl<W: for<'writer> MakeWriter<'writer>> RequestLog<W> {
    fn write(&self, text: &str) {
        // One `write_all` per unit: `Stdout` holds its lock for the whole call.
        drop(self.writer.make_writer().write_all(text.as_bytes()));
    }

    fn paint(&self, color: &str, text: &str) -> String {
        if self.colors {
            format!("\x1b[{color}m{text}\x1b[0m")
        } else {
            text.to_owned()
        }
    }

    fn level(&self, level: Level, width: usize) -> String {
        let name = if level == Level::INFO {
            String::new()
        } else {
            level.to_string()
        };
        let padded = format!("{name:<width$}");
        match level {
            Level::ERROR => self.paint("31", &padded),
            Level::WARN => self.paint("33", &padded),
            _ => self.paint("2", &padded),
        }
    }

    /// An event outside any request or task: one line.
    fn line(&self, entry: &Entry) -> String {
        format!(
            "{} {} {}{}\n",
            self.paint("2", &clock(entry.at)),
            self.level(entry.level, 5),
            sanitize(&entry.message),
            pairs(&entry.fields),
        )
    }

    /// A `pretty` block.
    fn block(&self, unit: &Unit) -> String {
        let fields = &unit.fields;
        let field = |name| fields.get(name).unwrap_or_default().into_owned();
        let (title, tag) = match unit.kind {
            UnitKind::Request => (
                format!("{} {}", field("method"), sanitize(&field("path"))),
                format!("req {}", short(&field("request_id"))),
            ),
            UnitKind::Task => (format!("task {}", field("task")), String::new()),
        };
        let mut block = String::new();
        let head = format!("┌─ {}  {title}", clock(unit.started));
        let _ = writeln!(
            block,
            "{}",
            justify(&head, &self.paint("2", &tag), tag.len())
        );

        let mut row = |label: &str, value: &str| {
            let _ = writeln!(block, "│  {label:<LABEL$}{}", sanitize(value));
        };
        for (label, name) in [
            ("client", "client"),
            ("caller", "caller"),
            ("idempotency-key", "idempotency_key"),
        ] {
            if let Some(value) = fields.get(name) {
                row(label, &value);
            }
        }
        for entry in &unit.events {
            let _ = writeln!(
                block,
                "│  {} {}{}{}",
                self.paint("2", &clock(entry.at)),
                self.level(entry.level, LABEL - 13),
                sanitize(&entry.message),
                pairs(&entry.fields),
            );
        }
        if let Some(problem) = fields.get("problem") {
            let _ = writeln!(block, "│  {:<LABEL$}{}", "problem", sanitize(&problem));
        }

        let (outcome, color) = match unit.kind {
            UnitKind::Request => match status(fields) {
                Some(status) => (
                    format!(
                        "{} {}",
                        status.as_u16(),
                        status.canonical_reason().unwrap_or_default()
                    ),
                    status_color(status),
                ),
                None => ("no response: the client went away".to_owned(), "33"),
            },
            UnitKind::Task => match field("outcome").as_str() {
                "failed" => ("failed".to_owned(), "31"),
                "more work" => ("done, more waiting".to_owned(), "32"),
                _ => ("done".to_owned(), "32"),
            },
        };
        let latency = format_latency(latency(unit));
        let foot = format!("└─ {}", self.paint(color, &outcome));
        let foot_len = "└─ ".chars().count() + outcome.chars().count();
        let _ = writeln!(
            block,
            "{foot}{}{latency}",
            " ".repeat(
                WIDTH
                    .saturating_sub(foot_len + latency.chars().count())
                    .max(1)
            )
        );
        block.push('\n');
        block
    }

    /// A `compact` line: the request, then its warnings and errors. Never colored.
    fn summary(unit: &Unit) -> String {
        let fields = &unit.fields;
        let field = |name| fields.get(name).unwrap_or_default().into_owned();
        let mut line = format!("{} ", clock(unit.started));
        match unit.kind {
            UnitKind::Request => {
                let status =
                    status(fields).map_or_else(|| "---".to_owned(), |s| s.as_str().to_owned());
                let _ = write!(
                    line,
                    "{status} {} {} {} req={}",
                    field("method"),
                    sanitize(&field("path")),
                    format_latency(latency(unit)),
                    short(&field("request_id")),
                );
                for name in ["caller", "client", "idempotency_key", "problem"] {
                    if let Some(value) = fields.get(name) {
                        let _ = write!(line, " {name}={}", quote(&value));
                    }
                }
            }
            UnitKind::Task => {
                let _ = write!(line, "task {} {}", field("task"), field("outcome"));
            }
        }
        for entry in &unit.events {
            let shown = unit.kind == UnitKind::Task || entry.level <= Level::WARN;
            if shown {
                let _ = write!(
                    line,
                    " | {}{}",
                    sanitize(&entry.message),
                    pairs(&entry.fields)
                );
            }
        }
        line.push('\n');
        line
    }
}

/// `head` and `tail` on one line, `tail` flush with [`WIDTH`]. `tail_len` is its visible length.
fn justify(head: &str, tail: &str, tail_len: usize) -> String {
    if tail_len == 0 {
        return head.to_owned();
    }
    let gap = WIDTH.saturating_sub(head.chars().count() + tail_len).max(1);
    format!("{head}{}{tail}", " ".repeat(gap))
}

fn status(fields: &Fields) -> Option<StatusCode> {
    fields
        .number("status")
        .and_then(|status| u16::try_from(status).ok())
        .and_then(|status| StatusCode::from_u16(status).ok())
}

fn status_color(status: StatusCode) -> &'static str {
    if status.is_server_error() {
        "31"
    } else if status.is_client_error() {
        "33"
    } else {
        "32"
    }
}

fn latency(unit: &Unit) -> Duration {
    unit.fields.number("latency_us").map_or_else(
        || unit.started.elapsed().unwrap_or_default(),
        Duration::from_micros,
    )
}

fn format_latency(latency: Duration) -> String {
    let micros = latency.as_micros();
    if micros < 1_000 {
        format!("{micros} µs")
    } else if micros < 1_000_000 {
        format!("{:.1} ms", latency.as_secs_f64() * 1e3)
    } else {
        format!("{:.2} s", latency.as_secs_f64())
    }
}

/// UTC time of day with milliseconds, e.g. `10:00:01.204`.
fn clock(at: SystemTime) -> String {
    let since_epoch = at.duration_since(UNIX_EPOCH).unwrap_or_default();
    let seconds = since_epoch.as_secs() % 86_400;
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        seconds / 3_600,
        seconds / 60 % 60,
        seconds % 60,
        since_epoch.subsec_millis()
    )
}

fn timestamp(at: SystemTime) -> String {
    let micros = at
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros();
    Timestamp::from_unix_micros(i64::try_from(micros).unwrap_or(i64::MAX)).to_string()
}

/// The first eight characters of a request id: enough to tell requests apart.
fn short(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

/// Escapes control characters, so a logged value cannot forge log lines.
fn sanitize(text: &str) -> String {
    if text.chars().any(char::is_control) {
        text.chars()
            .flat_map(|c| {
                if c.is_control() {
                    c.escape_default().collect::<Vec<_>>()
                } else {
                    vec![c]
                }
            })
            .collect()
    } else {
        text.to_owned()
    }
}

fn quote(value: &str) -> String {
    if value.contains([' ', '"', '=']) || value.is_empty() {
        format!("{value:?}")
    } else {
        sanitize(value)
    }
}

/// `  key=value key=value`, or nothing.
fn pairs(fields: &Fields) -> String {
    let mut text = String::new();
    for (index, (key, value)) in fields.0.iter().enumerate() {
        let gap = if index == 0 { "  " } else { " " };
        let _ = write!(text, "{gap}{key}={}", quote(&self::text(value)));
    }
    text
}

fn json_event(entry: &Entry, tag: Option<(&str, String)>) -> String {
    let mut object = serde_json::Map::new();
    object.insert("timestamp".into(), timestamp(entry.at).into());
    object.insert("level".into(), entry.level.as_str().into());
    object.insert("target".into(), entry.target.clone().into());
    if let Some((key, value)) = tag {
        object.insert(key.into(), value.into());
    }
    object.insert("message".into(), entry.message.clone().into());
    for (key, value) in &entry.fields.0 {
        object.insert((*key).into(), value.clone());
    }
    format!("{}\n", Value::Object(object))
}

fn json_request(unit: &Unit) -> String {
    let mut object = serde_json::Map::new();
    object.insert("timestamp".into(), timestamp(SystemTime::now()).into());
    object.insert("level".into(), "INFO".into());
    object.insert("target".into(), "kippu::request".into());
    object.insert("message".into(), "request finished".into());
    for (key, value) in &unit.fields.0 {
        object.insert((*key).into(), value.clone());
    }
    format!("{}\n", Value::Object(object))
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tracing::field::Empty;

    use super::*;

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

    impl<'writer> MakeWriter<'writer> for Buffer {
        type Writer = Self;

        fn make_writer(&'writer self) -> Self {
            self.clone()
        }
    }

    /// Logs a request the way `kippu-core` does, plus a stray event and an idle task run.
    fn capture(format: LogFormat) -> String {
        let buffer = Buffer::default();
        let subscriber =
            tracing_subscriber::registry().with(RequestLog::new(format, buffer.clone()));
        tracing::subscriber::with_default(subscriber, || {
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
                status = Empty,
                latency_us = Empty,
            );
            let other = tracing::info_span!(REQUEST_SPAN, method = "GET", path = "/healthz");
            request.in_scope(|| {
                tracing::Span::current().record("caller", "account:0199 (user)");
                other.in_scope(|| tracing::info!("interleaved"));
                tracing::info!(id = "6ba1", "purchase request queued");
                tracing::warn!("evil\nINFO forged line");
            });
            request.record("status", 202_u16);
            request.record("latency_us", 4_800_u64);
            drop(other);
            drop(request);
            let task = tracing::info_span!(TASK_SPAN, task = "expiry", outcome = Empty);
            task.record("outcome", "idle");
            drop(task);
        });
        let bytes = buffer.0.lock().unwrap().clone();
        String::from_utf8(bytes).unwrap()
    }

    #[test]
    fn pretty_writes_a_block_per_request() {
        let output = capture(LogFormat::Pretty);
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
        assert!(foot.starts_with("└─ 202 Accepted"), "{foot}");
        assert!(foot.ends_with("4.8 ms"), "{foot}");
        assert_eq!(foot.chars().count(), WIDTH, "{foot}");
        assert!(
            !output.contains("expiry"),
            "idle task runs are not shown: {output}"
        );
        assert!(output.ends_with("\n\n"));
    }

    #[test]
    fn compact_writes_a_line_per_request() {
        let output = capture(LogFormat::Compact);
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 3, "{output}");
        let line = lines[2];
        assert!(
            line.contains("202 POST /v1/sales/0199/purchase-requests 4.8 ms req=5cdc82aa"),
            "{line}"
        );
        assert!(line.contains("caller=\"account:0199 (user)\""), "{line}");
        assert!(line.contains("| evil\\nINFO forged line"), "{line}");
        assert!(!line.contains("purchase request queued"), "{line}");
    }

    #[test]
    fn json_writes_an_object_per_event() {
        let output = capture(LogFormat::Json);
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
        let finished = lines
            .iter()
            .find(|line| line["message"] == "request finished" && line["method"] == "POST")
            .unwrap();
        assert_eq!(finished["status"], 202);
        assert_eq!(finished["latency_us"], 4_800);
        assert_eq!(finished["caller"], "account:0199 (user)");
        assert!(!output.contains("expiry"));
    }
}
