use kippu_domain::account::{Account, Contacts};
use kippu_domain::purchase::LineItem;
use kippu_domain::reservation::ReservationStatus;
use kippu_domain::{AccountId, AttestorId, Timestamp};
use kippu_store::{Keyset, PageRequest};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::error::ApiError;
use crate::http::{ListingTag, PageQuery};

/// What to buy. Send it with an `Idempotency-Key` header; retrying with the same key is safe.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PurchaseRequestBody {
    /// Ticket types and quantities.
    pub items: Vec<LineItem>,
}

/// Tickets to grant. Send it with an `Idempotency-Key` header; retrying with the same key is
/// safe.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct GrantRequest {
    /// The account that gets the tickets, as its holder tells you.
    pub account_id: AccountId,
    /// Ticket types and quantities.
    pub items: Vec<LineItem>,
}

/// Choose how to pay for a reservation.
#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CheckoutRequest {
    /// The attestor (payment method) to pay with. Omit for free reservations, which are
    /// settled immediately.
    pub attestor_id: Option<AttestorId>,
}

/// Where a purchase request is polled; carries a receipt when the deployment queues requests
/// before persisting them.
#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in = Query)]
pub struct PollQuery {
    /// The receipt from the `Location` the request was accepted with.
    pub receipt: Option<String>,
}

/// Which of an event's reservations to list, and which page.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in = Query)]
pub struct EventReservationsQuery {
    /// Only reservations in this status, e.g. `issued` or `payment_pending`.
    pub status: Option<ReservationStatus>,
    /// At most this many reservations (default 50, at most 200).
    pub limit: Option<u32>,
    /// The `next_cursor` of the previous page. Keep the other parameters as they were.
    pub cursor: Option<String>,
}

impl EventReservationsQuery {
    /// The status to list and the page.
    pub(crate) fn into_parts(
        self,
    ) -> Result<(Option<ReservationStatus>, PageRequest<Keyset>), ApiError> {
        let page = PageQuery {
            limit: self.limit,
            cursor: self.cursor,
        }
        .page(ListingTag::EVENT_RESERVATIONS)?;
        Ok((self.status, page))
    }
}

/// Someone who ordered for an event, as its organizers see them: no email, only what they
/// chose to share.
#[derive(Debug, Serialize, ToSchema)]
pub struct BuyerView {
    /// The buyer's account, as reservations and denials name it.
    pub account_id: AccountId,
    /// Their name; empty if they closed their account.
    pub display_name: String,
    /// Ways to reach them, as they named them.
    pub contacts: Contacts,
    /// When they closed their account, if they did.
    pub deleted_at: Option<Timestamp>,
}

impl From<Account> for BuyerView {
    fn from(account: Account) -> Self {
        Self {
            account_id: account.id,
            display_name: account.display_name,
            contacts: account.contacts,
            deleted_at: account.deleted_at,
        }
    }
}
