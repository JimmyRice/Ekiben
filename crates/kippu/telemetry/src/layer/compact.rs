//! The `compact` format: a line per request or task run.

use std::fmt::Write as _;

use tracing::Level;

use super::text::{clock, format_latency, latency, pairs, quote, sanitize, short, status};
use super::unit::{Unit, UnitKind};

/// A `compact` line: the request, then its warnings and errors. Never colored.
pub(super) fn line(unit: &Unit) -> String {
    let fields = &unit.fields;
    let field = |name| fields.get(name).unwrap_or_default().into_owned();
    let mut line = format!("{} ", clock(unit.started));
    match unit.kind {
        UnitKind::Request => {
            let status = status(fields).map_or_else(|| "---".to_owned(), |s| s.as_str().to_owned());
            let _ = write!(
                line,
                "{status} {} {} {} req={}",
                field("method"),
                sanitize(&field("path")),
                format_latency(latency(unit)),
                short(&field("request_id")),
            );
            for name in ["caller", "client", "idempotency_key", "problem", "origin"] {
                if let Some(value) = fields.get(name) {
                    let _ = write!(line, " {name}={}", quote(&value));
                }
            }
            if unit.queries.count > 0 {
                let _ = write!(
                    line,
                    " db={}/{}",
                    unit.queries.count,
                    format_latency(unit.queries.time).replace(' ', "")
                );
            }
        }
        UnitKind::Task => {
            let _ = write!(line, "task {} {}", field("task"), field("outcome"));
        }
    }
    if status(fields).is_some_and(|status| status.as_u16() >= 400) {
        let steps: Vec<&str> = unit.steps().map(|step| step.label.as_str()).collect();
        if !steps.is_empty() {
            let _ = write!(line, " trace={}", quote(&steps.join(">")));
        }
    }
    for entry in unit.entries() {
        if unit.kind == UnitKind::Task || entry.level <= Level::WARN {
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
