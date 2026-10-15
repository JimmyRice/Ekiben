//! Invitations: the accounts a sale for invited accounts only is open to.
//!
//! Organizers keep a list per sale, e.g. the cosplayers whose application they approved. A
//! sale whose audience is [`Invited`](crate::catalog::Audience::Invited) is seen, and bought
//! from, only by the accounts on its list (and by its organizers). Leaving the list stops an
//! account from buying more; tickets it bought stay valid.

use serde::{Deserialize, Serialize};

use crate::{AccountId, SaleId, Timestamp};

/// An account on a sale's invitation list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Invitation {
    /// The sale.
    pub sale_id: SaleId,
    /// The invited account.
    pub account_id: AccountId,
    /// When it was invited.
    pub created_at: Timestamp,
}
