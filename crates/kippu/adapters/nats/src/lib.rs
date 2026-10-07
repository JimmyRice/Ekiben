#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

mod bus;
mod inbox;
mod queue;

pub use bus::SEQUENCE_HEADER;
pub use queue::{NatsOptions, NatsQueue};
