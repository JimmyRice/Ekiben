#![doc = include_str!("../README.md")]

mod error;
mod ticket;
mod verifier;

pub use error::KaisatsuError;
pub use ticket::{Extension, Ticket};
pub use verifier::Verifier;

uniffi::setup_scaffolding!();
