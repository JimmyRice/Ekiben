//! What a `request` or `task` span collects: its fields, its call chain and its events.

use std::time::{Duration, Instant, SystemTime};

use tracing::Level;

use super::collect::Fields;

/// Statements `sqlx` runs are counted, not listed: a request can run dozens.
pub(super) const QUERY_TARGET: &str = "sqlx::query";

/// A statement run this often in one request is named as a likely N+1.
pub(super) const REPEATED: u32 = 5;

/// What a `request` or `task` span has collected so far.
pub(super) struct Unit {
    pub(super) kind: UnitKind,
    pub(super) started: SystemTime,
    pub(super) fields: Fields,
    pub(super) items: Vec<Item>,
    pub(super) queries: Queries,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum UnitKind {
    Request,
    Task,
}

/// A line of a block, in the order it happened.
pub(super) enum Item {
    Step(Step),
    Event(Entry),
}

/// One step of the call chain: an instrumented function, or a call to something outside.
pub(super) struct Step {
    pub(super) at: SystemTime,
    pub(super) depth: usize,
    pub(super) kind: StepKind,
    pub(super) label: String,
    /// Filled in when the span closes.
    pub(super) elapsed: Option<Duration>,
    /// The error is on its way out of this step (or it failed itself).
    pub(super) failed: bool,
    /// Why a call to something outside failed.
    pub(super) error: Option<String>,
    /// Statements run directly in this step.
    pub(super) queries: Queries,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum StepKind {
    /// An instrumented function.
    Service,
    /// A call to something outside the process.
    Dependency,
}

/// Marks a span nested in a unit; kept in the span's extensions.
pub(super) struct StepMark {
    /// Position in the unit's items, unless the format shows no steps.
    pub(super) index: Option<usize>,
    pub(super) label: String,
    pub(super) started: Instant,
}

/// Database statements: how many, how long, and which one repeats most.
#[derive(Default)]
pub(super) struct Queries {
    pub(super) count: u32,
    pub(super) time: Duration,
    repeats: Vec<(String, u32)>,
}

impl Queries {
    pub(super) fn add(&mut self, summary: &str, time: Duration) {
        self.count += 1;
        self.time += time;
        if let Some((_, count)) = self.repeats.iter_mut().find(|(seen, _)| seen == summary) {
            *count += 1;
        } else if self.repeats.len() < 64 {
            // A request that runs this many different statements has no N+1 to find.
            self.repeats.push((summary.to_owned(), 1));
        }
    }

    /// The statement run most often, when it is run often enough to be suspicious.
    pub(super) fn repeated(&self) -> Option<(&str, u32)> {
        self.repeats
            .iter()
            .filter(|(_, count)| *count >= REPEATED)
            .max_by_key(|(_, count)| *count)
            .map(|(summary, count)| (summary.as_str(), *count))
    }
}

/// One logged event.
pub(super) struct Entry {
    pub(super) at: SystemTime,
    pub(super) level: Level,
    pub(super) target: String,
    /// Source file and line of the logging call.
    pub(super) location: Option<String>,
    /// How many steps deep the event happened.
    pub(super) depth: usize,
    pub(super) message: String,
    pub(super) fields: Fields,
}

impl Unit {
    pub(super) fn entries(&self) -> impl Iterator<Item = &Entry> {
        self.items.iter().filter_map(|item| match item {
            Item::Event(entry) => Some(entry),
            Item::Step(_) => None,
        })
    }

    pub(super) fn steps(&self) -> impl Iterator<Item = &Step> {
        self.items.iter().filter_map(|item| match item {
            Item::Step(step) => Some(step),
            Item::Event(_) => None,
        })
    }

    /// Marks the steps an error passed through: the one that raised it (`origin` is
    /// `step @ file:line`) and every step it was called from.
    pub(super) fn mark_failure(&mut self) {
        let Some(origin) = self.fields.get("origin") else {
            return;
        };
        let Some((name, _)) = origin.split_once(" @ ") else {
            return;
        };
        let Some(raised) = self.items.iter().rposition(|item| {
            matches!(item, Item::Step(step)
                if step.kind == StepKind::Service
                    && step.label.rsplit("::").next() == Some(name))
        }) else {
            return;
        };
        let mut depth = usize::MAX;
        for item in self.items[..=raised].iter_mut().rev() {
            if let Item::Step(step) = item
                && step.depth < depth
            {
                step.failed = true;
                depth = step.depth;
            }
        }
    }
}

/// The module and function of a step, e.g. `purchasing::buying::submit_purchase`.
pub(super) fn step_label(target: &str, name: &str) -> String {
    let path = target
        .split_once("modules::")
        .map_or(target, |(_, rest)| rest);
    let parts: Vec<&str> = path.split("::").filter(|part| *part != "service").collect();
    let start = parts.len().saturating_sub(2);
    let mut label = parts.get(start..).unwrap_or_default().join("::");
    if !label.is_empty() {
        label.push_str("::");
    }
    label.push_str(name);
    label
}
