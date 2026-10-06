//! The purchase pipeline, independent of HTTP.
//!
//! ```text
//! submit ──▶ Queued (durable) ──worker──▶ Reserved (inventory held) ──checkout──▶ PaymentPending
//!                                  └────▶ Rejected (sold out, limit, …)
//! ```
//!
//! Submitting only records intent, so the front door stays fast under any load; workers drain
//! the queue at a steady pace, and the database alone decides who gets a ticket. Buyers' use cases
//! are in `buying.rs`, the workers in `pipeline.rs`.

mod buying;
mod pipeline;

pub use buying::*;
pub(crate) use pipeline::{expire_batch, process_batch};
