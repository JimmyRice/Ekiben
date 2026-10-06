//! Small formatting helpers shared by the formats.

use std::fmt::Write as _;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use http::StatusCode;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use super::collect::{Fields, text};
use super::unit::Unit;

pub(super) fn status(fields: &Fields) -> Option<StatusCode> {
    fields
        .number("status")
        .and_then(|status| u16::try_from(status).ok())
        .and_then(|status| StatusCode::from_u16(status).ok())
}

pub(super) fn latency(unit: &Unit) -> Duration {
    unit.fields.number("latency_us").map_or_else(
        || unit.started.elapsed().unwrap_or_default(),
        Duration::from_micros,
    )
}

pub(super) fn format_latency(latency: Duration) -> String {
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
pub(super) fn clock(at: SystemTime) -> String {
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

/// RFC 3339 with microseconds.
pub(super) fn timestamp(at: SystemTime) -> String {
    let micros = at
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros();
    i128::try_from(micros)
        .ok()
        .and_then(|micros| OffsetDateTime::from_unix_timestamp_nanos(micros * 1_000).ok())
        .and_then(|at| at.format(&Rfc3339).ok())
        .unwrap_or_default()
}

/// The first eight characters of a request id: enough to tell requests apart.
pub(super) fn short(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

/// Escapes control characters, so a logged value cannot forge log lines.
pub(super) fn sanitize(text: &str) -> String {
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

pub(super) fn quote(value: &str) -> String {
    if value.contains([' ', '"', '=']) || value.is_empty() {
        format!("{value:?}")
    } else {
        sanitize(value)
    }
}

/// `  key=value key=value`, or nothing.
pub(super) fn pairs(fields: &Fields) -> String {
    let mut out = String::new();
    for (index, (key, value)) in fields.0.iter().enumerate() {
        let gap = if index == 0 { "  " } else { " " };
        let _ = write!(out, "{gap}{key}={}", quote(&text(value)));
    }
    out
}

/// `3 queries, 0.4 ms`.
pub(super) fn describe_queries(count: u32, time: Duration) -> String {
    let noun = if count == 1 { "query" } else { "queries" };
    format!("{count} {noun}, {}", format_latency(time))
}
