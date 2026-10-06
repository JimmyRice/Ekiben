//! The `json` format: an object per event, and one when a request finishes.

use std::time::SystemTime;

use serde_json::{Map, Value};
use tracing::Level;

use super::text::timestamp;
use super::unit::{Entry, Unit};

/// An event, tagged with the request or task it belongs to and the step it happened in.
pub(super) fn event(entry: &Entry, tag: Option<(&str, String)>, step: Option<&str>) -> String {
    let mut object = Map::new();
    object.insert("timestamp".into(), timestamp(entry.at).into());
    object.insert("level".into(), entry.level.as_str().into());
    object.insert("target".into(), entry.target.clone().into());
    if let Some((key, value)) = tag {
        object.insert(key.into(), value.into());
    }
    if let Some(step) = step {
        object.insert("step".into(), step.into());
    }
    if let (Some(location), true) = (&entry.location, entry.level <= Level::WARN) {
        object.insert("at".into(), location.clone().into());
    }
    object.insert("message".into(), entry.message.clone().into());
    for (key, value) in &entry.fields.0 {
        object.insert((*key).into(), value.clone());
    }
    format!("{}\n", Value::Object(object))
}

/// The `request finished` line: the request's fields, and what it asked of the database.
pub(super) fn finished(unit: &Unit) -> String {
    let mut object = Map::new();
    object.insert("timestamp".into(), timestamp(SystemTime::now()).into());
    object.insert("level".into(), "INFO".into());
    object.insert("target".into(), "kippu::request".into());
    object.insert("message".into(), "request finished".into());
    for (key, value) in &unit.fields.0 {
        object.insert((*key).into(), value.clone());
    }
    if unit.queries.count > 0 {
        object.insert("db_queries".into(), unit.queries.count.into());
        object.insert(
            "db_us".into(),
            u64::try_from(unit.queries.time.as_micros())
                .unwrap_or(u64::MAX)
                .into(),
        );
    }
    format!("{}\n", Value::Object(object))
}
