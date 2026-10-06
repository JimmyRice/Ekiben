//! HTTP assembly: module routes, health checks, OpenAPI and cross-cutting middleware.

mod body_limit;
mod health;
pub mod idempotency;
mod json;
mod openapi;
mod paging;
mod problems;
mod router;
pub mod trace;

pub use body_limit::RequestBodyLimit;
pub use json::Json;
pub use paging::PageQuery;

pub(crate) use router::router;
