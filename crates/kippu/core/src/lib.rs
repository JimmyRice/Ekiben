#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

pub mod app;
pub mod auth;
pub mod clock;
pub mod config;
pub mod error;
pub mod http;
pub mod keys;
mod messaging;
pub mod module;
pub mod modules;
mod workers;

pub use app::{App, AppState, Kippu};
pub use auth::{Permission, Principal, Scope};
pub use clock::{Clock, ManualClock, SystemClock};
pub use config::Config;
pub use error::{ApiError, ApiResult, ProblemKind};
pub use module::{BackgroundTask, BodyLimit, IdempotentRoute, Module, Progress};
pub use modules::default_modules;
