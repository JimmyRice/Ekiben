//! The modules a deployment runs unless it chooses otherwise.

use std::sync::Arc;

use super::{
    accounts, admission, catalog, denials, images, payments, purchasing, ticketing, webhooks,
};
use crate::module::Module;

/// Every built-in module, in the order their routes are documented.
pub fn default_modules() -> Vec<Arc<dyn Module>> {
    vec![
        Arc::new(accounts::Accounts),
        Arc::new(catalog::Catalog),
        Arc::new(images::Images),
        Arc::new(admission::Admission),
        Arc::new(purchasing::Purchasing),
        Arc::new(payments::Payments),
        Arc::new(ticketing::Ticketing),
        Arc::new(denials::Denials),
        Arc::new(webhooks::Webhooks),
    ]
}
