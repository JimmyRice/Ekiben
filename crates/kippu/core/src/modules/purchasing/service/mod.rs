//! The purchase pipeline, independent of HTTP.
//!
//! ```text
//! submit ──▶ Queued (durable) ──worker──▶ Reserved (inventory held) ──checkout──▶ PaymentPending
//!                                  └────▶ Rejected (sold out, limit, …)
//! ```
//!
//! Submitting only records intent, so the front door stays fast under any load; workers drain
//! the queue at a steady pace, and the database alone decides who gets a ticket. Buyers' use cases
//! are in `buying.rs`, what organizers read in `orders.rs`, the workers in `pipeline.rs`, and
//! organizers' grants of a private sale's tickets, which skip the queue, in `granting.rs`.

mod buying;
mod granting;
mod orders;
mod pipeline;

pub use buying::*;
pub use granting::*;
pub use orders::*;
pub(crate) use pipeline::{expire_batch, process_batch, release};
