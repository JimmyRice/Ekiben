//! Webhook use cases, independent of HTTP: managing webhooks (`management.rs`) and delivering
//! events to them (`dispatch.rs`).

mod dispatch;
mod management;

pub(crate) use dispatch::delivery_task;
pub use management::*;
