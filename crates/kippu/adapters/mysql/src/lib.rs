#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

mod accounts;
mod catalog;
mod convert;
mod denials;
mod errors;
mod housekeeping;
mod images;
mod outbox;
mod payments;
mod purchasing;
mod store;
mod ticketing;
mod tx;
mod webhooks;

pub use store::MySqlStore;

pub(crate) use errors::{error, is_duplicate, unique};
