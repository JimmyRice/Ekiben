#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

pub mod account;
pub mod admission;
pub mod catalog;
pub mod id;
pub mod money;
pub mod outbox;
pub mod payment;
pub mod purchase;
pub mod reservation;
pub mod ticket;
mod timestamp;
pub mod validation;
pub mod webhook;

pub use id::*;
pub use money::{Currency, Money};
pub use timestamp::{Duration, ParseTimestampError, Timestamp};
pub use validation::ValidationError;
