//! Where something went wrong.

use std::fmt;
use std::panic::Location;

use tracing::Span;

use crate::{DEPENDENCY_SPAN, REQUEST_SPAN, TASK_SPAN};

/// The source line that raised an error and the innermost step (an instrumented function)
/// that was running. Logged next to the error; never part of a response.
///
/// Errors capture it where they are built, so a constructor that should point at its caller
/// needs `#[track_caller]` all the way up.
#[derive(Debug, Clone, Copy)]
pub struct Origin {
    location: &'static Location<'static>,
    step: Option<&'static str>,
}

impl Origin {
    /// Captures the caller's source line and the current step.
    #[track_caller]
    pub fn capture() -> Self {
        Self {
            location: Location::caller(),
            step: Span::current()
                .metadata()
                .map(tracing::Metadata::name)
                .filter(|name| !matches!(*name, REQUEST_SPAN | TASK_SPAN | DEPENDENCY_SPAN)),
        }
    }
}

/// `step @ file:line`, or just `file:line` outside any step.
impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(step) = self.step {
            write!(f, "{step} @ ")?;
        }
        write!(f, "{}:{}", self.location.file(), self.location.line())
    }
}
