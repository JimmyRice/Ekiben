//! The log layer: one unit of output per request or task run, whatever the format.
//!
//! [`LogLayer`] collects the events and nested spans of every `request` and `task` span (see
//! the crate's contract) and writes them out together when the span closes, so concurrent
//! requests never interleave:
//!
//! - `pretty` (`pretty.rs`): a block per request with its call chain; task runs get a small
//!   block too, unless they found nothing to do.
//! - `compact` (`compact.rs`): one line per request; failed ones add the steps they ran.
//! - `json` (`json.rs`): one object per line and per event, plus a `request finished` line.
//!
//! `unit.rs` holds what a span collects, `log.rs` the layer that collects it, `collect.rs`
//! field values, `text.rs` the small formatting helpers and `install.rs` the global setup.
//!
//! Pretty and compact output show a request once it is finished: a stuck request shows up
//! when the request timeout ends it with 408.

mod collect;
mod compact;
mod format;
mod install;
mod json;
mod log;
mod pretty;
#[cfg(test)]
mod tests;
mod text;
mod unit;

pub use format::Format;
pub use install::init;
pub use log::LogLayer;
