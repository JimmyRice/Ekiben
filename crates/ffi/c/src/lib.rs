#![doc = include_str!("../README.md")]
#![no_std]

#[cfg(feature = "std")]
extern crate std;

mod abi;
mod limits;
mod panic;
mod types;

pub use abi::*;
pub use limits::KAISATSU_MAX_TICKET_LEN;
pub use types::{KaisatsuKey, KaisatsuStatus, KaisatsuTicket};
