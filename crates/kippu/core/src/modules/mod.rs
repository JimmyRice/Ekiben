//! The built-in modules.

pub mod accounts;
pub mod admission;
pub mod catalog;
pub mod payments;
pub mod purchasing;
pub mod ticketing;
pub mod webhooks;

use std::sync::Arc;

use crate::module::Module;

/// Every built-in module, in the order their routes are documented.
pub fn default_modules() -> Vec<Arc<dyn Module>> {
    vec![
        Arc::new(accounts::Accounts),
        Arc::new(catalog::Catalog),
        Arc::new(admission::Admission),
        Arc::new(purchasing::Purchasing),
        Arc::new(payments::Payments),
        Arc::new(ticketing::Ticketing),
        Arc::new(webhooks::Webhooks),
    ]
}
