#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

mod adapter;
mod connect;

pub use adapter::ObjectStoreAdapter;
pub use connect::{Settings, connect};
