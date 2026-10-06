#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

pub mod adapters;
pub mod cli;
mod commands;
pub mod config;
mod launcher;
mod serving;
mod verify;

pub use launcher::Launcher;
pub use serving::{assemble, serve_app};
