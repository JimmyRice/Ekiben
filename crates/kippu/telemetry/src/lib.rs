#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

mod contract;
mod dependency;
#[cfg(feature = "layer")]
pub mod layer;
mod origin;

pub use contract::{
    DEPENDENCY_SPAN, REQUEST_SPAN, TASK_SPAN, record_caller, record_outcome, record_problem,
    record_response, request_span, task_span,
};
pub use dependency::call;
pub use origin::Origin;
