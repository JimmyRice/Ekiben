use kippu_domain::AttestorId;
use kippu_domain::purchase::LineItem;
use kippu_domain::reservation::ReservationStatus;
use kippu_store::{Keyset, PageRequest};
use serde::Deserialize;
use utoipa::{IntoParams, ToSchema};

use crate::error::ApiError;
use crate::http::{ListingTag, PageQuery};

/// What to buy. Send it with an `Idempotency-Key` header; retrying with the same key is safe.
#[derive(Debug, Deserialize, ToSchema)]
pub struct PurchaseRequestBody {
    /// Ticket types and quantities.
    pub items: Vec<LineItem>,
}

/// Choose how to pay for a reservation.
#[derive(Debug, Default, Deserialize, ToSchema)]
pub struct CheckoutRequest {
    /// The attestor (payment method) to pay with. Omit for free reservations, which are
    /// settled immediately.
    pub attestor_id: Option<AttestorId>,
}

/// Where a purchase request is polled; carries a receipt when the deployment queues requests
/// before persisting them.
#[derive(Debug, Deserialize, utoipa::IntoParams)]
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
