//! The built-in modules.

pub mod accounts;

use std::sync::Arc;

use crate::module::Module;

/// Every built-in module, in the order their routes are documented.
pub fn default_modules() -> Vec<Arc<dyn Module>> {
    vec![Arc::new(accounts::Accounts)]
}
