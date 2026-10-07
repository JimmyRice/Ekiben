//! The `pretty` format: a block per request or task run.

use std::fmt::Write as _;

use http::StatusCode;
use tracing::Level;

use super::text::{
    clock, describe_queries, format_latency, latency, pairs, sanitize, short, status,
};
use super::unit::{Entry, Item, Step, StepKind, Unit, UnitKind};

/// Width of a block, not counting text that does not fit.
const WIDTH: usize = 80;
/// Width of the label column inside a block.
const LABEL: usize = 18;
/// Longest error shown after a failed call.
const ERROR_WIDTH: usize = 48;

/// Colors text when asked to.
pub(super) struct Paint(pub(super) bool);

impl Paint {
    fn paint(&self, color: &str, text: &str) -> String {
        if self.0 {
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
}

/// An event outside any request or task: one line.
pub(super) fn line(paint: &Paint, entry: &Entry) -> String {
    format!(
        "{} {} {}{}\n",
        paint.paint("2", &clock(entry.at)),
        paint.level(entry.level, 5),
        sanitize(&entry.message),
        pairs(&entry.fields),
    )
}

/// A `pretty` block.
pub(super) fn block(paint: &Paint, unit: &Unit) -> String {
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
    let gap = WIDTH
        .saturating_sub(head.chars().count() + tag.chars().count())
        .max(1);
    if tag.is_empty() {
        let _ = writeln!(block, "{head}");
    } else {
        let _ = writeln!(block, "{head}{}{}", " ".repeat(gap), paint.paint("2", &tag));
    }

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
    if unit.queries.count > 0 {
        let mut value = describe_queries(unit.queries.count, unit.queries.time);
        if let Some((summary, count)) = unit.queries.repeated() {
            let _ = write!(value, " · {count}× {summary}");
        }
        row("db", &value);
    }
    for item in &unit.items {
        match item {
            Item::Step(step) => {
                let _ = writeln!(block, "{}", step_row(paint, step));
            }
            Item::Event(entry) => {
                let _ = writeln!(
                    block,
                    "│  {} {}{}{}{}{}",
                    paint.paint("2", &clock(entry.at)),
                    paint.level(entry.level, LABEL - 13),
                    "  ".repeat(entry.depth),
                    sanitize(&entry.message),
                    pairs(&entry.fields),
                    location_of(paint, entry),
                );
            }
        }
    }
    for (label, name) in [("problem", "problem"), ("origin", "origin")] {
        if let Some(value) = fields.get(name) {
            let _ = writeln!(block, "│  {label:<LABEL$}{}", sanitize(&value));
        }
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
    let elapsed = format_latency(latency(unit));
    let foot_len = "└─ ".chars().count() + outcome.chars().count();
    let _ = writeln!(
        block,
        "└─ {}{}{elapsed}",
        paint.paint(color, &outcome),
        " ".repeat(
            WIDTH
                .saturating_sub(foot_len + elapsed.chars().count())
                .max(1)
        )
    );
    block.push('\n');
    block
}

/// A step of the call chain: what ran, what it asked of the database, how long it took.
fn step_row(paint: &Paint, step: &Step) -> String {
    let mark = match (step.failed, step.kind) {
        (true, _) => '✗',
        (false, StepKind::Service) => '▸',
        (false, StepKind::Dependency) => '◆',
    };
    let mut head = format!(
        "│  {}      {}{mark} {}",
        clock(step.at),
        "  ".repeat(step.depth),
        step.label
    );
    if let Some(error) = &step.error {
        let error = sanitize(error);
        let shown: String = error.chars().take(ERROR_WIDTH).collect();
        let ellipsis = if shown.len() < error.len() { "…" } else { "" };
        let _ = write!(head, "  {shown}{ellipsis}");
    }
    let mut tail = String::new();
    if step.queries.count > 0 {
        let _ = write!(
            tail,
            "({})  ",
            describe_queries(step.queries.count, step.queries.time)
        );
    }
    tail.push_str(&step.elapsed.map(format_latency).unwrap_or_default());
    let gap = WIDTH
        .saturating_sub(head.chars().count() + tail.chars().count())
        .max(1);
    let color = if step.failed { "31" } else { "2" };
    paint.paint(color, &format!("{head}{}{tail}", " ".repeat(gap)))
}

/// `  @ file:line` for warnings and errors: where the log call is.
fn location_of(paint: &Paint, entry: &Entry) -> String {
    match &entry.location {
        Some(location) if entry.level <= Level::WARN => {
            paint.paint("2", &format!("  @ {location}"))
        }
        _ => String::new(),
    }
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
