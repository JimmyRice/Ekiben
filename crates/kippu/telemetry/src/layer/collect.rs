//! Field values of spans and events.

use std::borrow::Cow;

use serde_json::Value;
use tracing::field::{Field, Visit};

/// Field values in recording order; recording a field again replaces its value. Values keep
/// their type (string, number, boolean) for JSON output.
#[derive(Default)]
pub(super) struct Fields(pub(super) Vec<(&'static str, Value)>);

impl Fields {
    /// A value as text.
    pub(super) fn get(&self, name: &str) -> Option<Cow<'_, str>> {
        self.value(name).map(text)
    }

    pub(super) fn number(&self, name: &str) -> Option<u64> {
        self.value(name).and_then(Value::as_u64)
    }

    pub(super) fn float(&self, name: &str) -> Option<f64> {
        self.value(name).and_then(Value::as_f64)
    }

    fn value(&self, name: &str) -> Option<&Value> {
        self.0
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value)
    }

    pub(super) fn set(&mut self, name: &'static str, value: Value) {
        match self.0.iter_mut().find(|(key, _)| *key == name) {
            Some((_, slot)) => *slot = value,
            None => self.0.push((name, value)),
        }
    }
}

/// Collects field values; `message` is kept apart.
#[derive(Default)]
pub(super) struct Collector {
    pub(super) message: Option<String>,
    pub(super) fields: Fields,
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
pub(super) fn text(value: &Value) -> Cow<'_, str> {
    match value {
        Value::String(text) => Cow::Borrowed(text),
        other => Cow::Owned(other.to_string()),
    }
}
