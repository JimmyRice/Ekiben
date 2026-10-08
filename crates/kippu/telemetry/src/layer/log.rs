//! The layer that collects spans and events into units.

use std::io::Write as _;
use std::time::{Duration, Instant, SystemTime};

use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::{LookupSpan, SpanRef};

use super::collect::Collector;
use super::format::Format;
use super::pretty::Paint;
use super::unit::{
    Entry, Item, QUERY_TARGET, Queries, Step, StepKind, StepMark, Unit, UnitKind, step_label,
};
use super::{compact, json, pretty};
use crate::{DEPENDENCY_SPAN, REQUEST_SPAN, TASK_SPAN};

/// A [`tracing_subscriber::Layer`] that writes each request and task run as one unit.
pub struct LogLayer<W> {
    format: Format,
    writer: W,
    colors: bool,
}

impl<W> LogLayer<W> {
    /// A layer writing `format` to `writer`.
    pub const fn new(format: Format, writer: W) -> Self {
        Self {
            format,
            writer,
            colors: false,
        }
    }

    /// Uses ANSI colors in `pretty` output.
    #[must_use]
    pub const fn colors(mut self, colors: bool) -> Self {
        self.colors = colors;
        self
    }

    /// Adds a span nested in a request or task run to its call chain.
    fn enter_step<S>(&self, span: &SpanRef<'_, S>, attrs: &Attributes<'_>)
    where
        S: Subscriber + for<'lookup> LookupSpan<'lookup>,
    {
        let mut depth = 0;
        let mut unit = None;
        for ancestor in span.scope().skip(1) {
            if ancestor.extensions().get::<Unit>().is_some() {
                unit = Some(ancestor);
                break;
            }
            if ancestor.extensions().get::<StepMark>().is_some() {
                depth += 1;
            }
        }
        let Some(unit) = unit else { return };
        let metadata = attrs.metadata();
        let (kind, label) = if metadata.name() == DEPENDENCY_SPAN {
            let mut collector = Collector::default();
            attrs.record(&mut collector);
            let part = |name| collector.fields.get(name).unwrap_or_default().into_owned();
            (
                StepKind::Dependency,
                format!("{} {}", part("system"), part("operation")),
            )
        } else {
            (
                StepKind::Service,
                step_label(metadata.target(), metadata.name()),
            )
        };
        let index = if self.format == Format::Json {
            None
        } else {
            unit.extensions_mut().get_mut::<Unit>().map(|unit| {
                unit.items.push(Item::Step(Step {
                    at: SystemTime::now(),
                    depth,
                    kind,
                    label: label.clone(),
                    elapsed: None,
                    failed: false,
                    error: None,
                    queries: Queries::default(),
                }));
                unit.items.len() - 1
            })
        };
        span.extensions_mut().insert(StepMark {
            index,
            label,
            started: Instant::now(),
        });
    }

    /// Records how long a step took.
    fn leave_step<S>(span: &SpanRef<'_, S>, mark: &StepMark)
    where
        S: Subscriber + for<'lookup> LookupSpan<'lookup>,
    {
        let Some(index) = mark.index else { return };
        let Some(unit) = enclosing_unit(span) else {
            return;
        };
        if let Some(Item::Step(step)) = unit
            .extensions_mut()
            .get_mut::<Unit>()
            .and_then(|unit| unit.items.get_mut(index))
        {
            step.elapsed = Some(mark.started.elapsed());
        }
    }
}

impl<W> LogLayer<W> {
    /// Adds a statement to the request or task it ran in, and to the step that ran it.
    fn count_statement<S>(event: &Event<'_>, ctx: &Context<'_, S>)
    where
        S: Subscriber + for<'lookup> LookupSpan<'lookup>,
    {
        let Some(scope) = ctx.event_scope(event) else {
            return;
        };
        let mut step_index = None;
        let mut unit = None;
        for span in scope {
            if span.extensions().get::<Unit>().is_some() {
                unit = Some(span);
                break;
            }
            if step_index.is_none() {
                step_index = span
                    .extensions()
                    .get::<StepMark>()
                    .and_then(|mark| mark.index);
            }
        }
        let Some(unit) = unit else { return };
        let mut statement = Statement::default();
        event.record(&mut statement);
        let time = Duration::from_secs_f64(statement.elapsed_secs);
        if let Some(unit) = unit.extensions_mut().get_mut::<Unit>() {
            unit.queries.add(&statement.summary, time);
            if let Some(Item::Step(step)) = step_index.and_then(|index| unit.items.get_mut(index)) {
                step.queries.add(&statement.summary, time);
            }
        }
    }
}

/// The two fields of a statement event that counting needs.
#[derive(Default)]
struct Statement {
    summary: String,
    elapsed_secs: f64,
}

impl tracing::field::Visit for Statement {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "summary" {
            value.clone_into(&mut self.summary);
        }
    }

    fn record_f64(&mut self, field: &tracing::field::Field, value: f64) {
        if field.name() == "elapsed_secs" {
            self.elapsed_secs = value;
        }
    }

    fn record_debug(&mut self, _: &tracing::field::Field, _: &dyn std::fmt::Debug) {}
}

impl<W: for<'writer> MakeWriter<'writer>> LogLayer<W> {
    fn write(&self, text: &str) {
        // One `write_all` per unit: `Stdout` holds its lock for the whole call.
        drop(self.writer.make_writer().write_all(text.as_bytes()));
    }
}

/// The request or task span a nested span runs in.
fn enclosing_unit<'a, S>(span: &SpanRef<'a, S>) -> Option<SpanRef<'a, S>>
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    span.scope()
        .skip(1)
        .find(|ancestor| ancestor.extensions().get::<Unit>().is_some())
}

impl<S, W> tracing_subscriber::Layer<S> for LogLayer<W>
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
    W: for<'writer> MakeWriter<'writer> + 'static,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else { return };
        let kind = match attrs.metadata().name() {
            REQUEST_SPAN => UnitKind::Request,
            TASK_SPAN => UnitKind::Task,
            _ => {
                self.enter_step(&span, attrs);
                return;
            }
        };
        let mut collector = Collector::default();
        attrs.record(&mut collector);
        span.extensions_mut().insert(Unit {
            kind,
            started: SystemTime::now(),
            fields: collector.fields,
            items: Vec::new(),
            queries: Queries::default(),
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
            return;
        }
        // A call to something outside failed: the span records `error`.
        let index = extensions.get_mut::<StepMark>().and_then(|mark| mark.index);
        drop(extensions);
        let (Some(index), Some(unit)) = (index, enclosing_unit(&span)) else {
            return;
        };
        let mut collector = Collector::default();
        values.record(&mut collector);
        let Some(error) = collector.fields.get("error") else {
            return;
        };
        if let Some(Item::Step(step)) = unit
            .extensions_mut()
            .get_mut::<Unit>()
            .and_then(|unit| unit.items.get_mut(index))
        {
            step.failed = true;
            step.error = Some(error.into_owned());
        }
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        // Statements are counted, not listed; only slow or failed ones (warnings) are events.
        // Counting reads two fields and copies nothing else: sqlx sends one event per
        // statement, with the whole SQL text.
        let metadata = event.metadata();
        if metadata.target().starts_with(QUERY_TARGET) && *metadata.level() > Level::WARN {
            Self::count_statement(event, &ctx);
            return;
        }
        let mut collector = Collector::default();
        event.record(&mut collector);
        let mut entry = Entry {
            at: SystemTime::now(),
            level: *metadata.level(),
            target: metadata.target().to_owned(),
            location: metadata
                .file()
                .zip(metadata.line())
                .map(|(file, line)| format!("{file}:{line}")),
            depth: 0,
            message: collector.message.unwrap_or_default(),
            fields: collector.fields,
        };
        let mut step = None;
        let mut unit = None;
        if let Some(scope) = ctx.event_scope(event) {
            for span in scope {
                if span.extensions().get::<Unit>().is_some() {
                    unit = Some(span);
                    break;
                }
                if let Some(mark) = span.extensions().get::<StepMark>() {
                    if step.is_none() {
                        step = Some(mark.label.clone());
                    }
                    entry.depth += 1;
                }
            }
        }

        match (self.format, unit) {
            (Format::Json, unit) => {
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
                self.write(&json::event(&entry, tag, step.as_deref()));
            }
            (_, Some(span)) => {
                if let Some(unit) = span.extensions_mut().get_mut::<Unit>() {
                    unit.items.push(Item::Event(entry));
                }
            }
            (Format::Pretty | Format::Compact, None) => {
                self.write(&pretty::line(&Paint(self.colors), &entry));
            }
        }
    }

    fn on_close(&self, id: Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(&id) else { return };
        let mark = span.extensions_mut().remove::<StepMark>();
        if let Some(mark) = mark {
            Self::leave_step(&span, &mark);
            return;
        }
        let Some(mut unit) = span.extensions_mut().remove::<Unit>() else {
            return;
        };
        let idle = unit.kind == UnitKind::Task
            && unit.entries().next().is_none()
            && unit.fields.get("outcome").as_deref() != Some("failed");
        if idle {
            return;
        }
        unit.mark_failure();
        let output = match (self.format, unit.kind) {
            (Format::Pretty, _) => pretty::block(&Paint(self.colors), &unit),
            (Format::Compact, _) => compact::line(&unit),
            (Format::Json, UnitKind::Request) => json::finished(&unit),
            (Format::Json, UnitKind::Task) => return,
        };
        self.write(&output);
    }
}
