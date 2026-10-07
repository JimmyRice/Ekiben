use kippu_domain::AttestorId;
use kippu_domain::purchase::LineItem;
use serde::Deserialize;
use utoipa::ToSchema;

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
