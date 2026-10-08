//! Reservations and their state machine.
//!
//! "Getting a ticket" first means getting a *reservation*: a temporary hold on inventory.
//! Only an attested payment turns it into tickets.
//!
//! ```text
//! Reserved ──checkout──▶ PaymentPending ──payment attested──▶ Issued
//!    │  └──── cancel ───▶ Cancelled  │
//!    └─────── TTL ───────────────────┴──▶ Expired ──late payment──┬─ stock left ─▶ Issued
//!                                                                 └─ sold out ──▶ RefundRequired ─▶ Refunded
//! ```
//!
//! Recording a payment and issuing its tickets happen in one database transaction, so "paid"
//! is never observable on its own — and money can never be taken without either tickets or
//! an explicit `RefundRequired`.

use serde::{Deserialize, Serialize};

use crate::payment::Environment;
use crate::{
    AccountId, AttestorId, EventId, Money, PurchaseRequestId, ReservationId, SaleId, TicketTypeId,
    Timestamp, ValidationError,
};

/// Where a reservation stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ReservationStatus {
    /// Inventory is held for the buyer.
    Reserved,
    /// The buyer started paying with an attestor; inventory is still held.
    PaymentPending,
    /// Paid; tickets are issued.
    Issued,
    /// The hold lapsed unpaid; inventory was released.
    Expired,
    /// The buyer gave up; inventory was released.
    Cancelled,
    /// Money arrived but no tickets could be issued; the attestor must refund.
    RefundRequired,
    /// The attestor confirmed the refund.
    Refunded,
}

/// What may not happen to a reservation in its current state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("cannot {action} a reservation that is {from:?}")]
pub struct IllegalTransition {
    /// The state the reservation is in.
    pub from: ReservationStatus,
    /// What was attempted.
    pub action: &'static str,
}

/// What to do with a payment that arrives for a reservation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Settlement {
    /// Inventory is still held: sell it and issue tickets.
    Issue,
    /// The hold lapsed: try to take the inventory again. If that succeeds, issue tickets;
    /// otherwise the reservation becomes `RefundRequired`.
    Reacquire,
    /// The reservation was already settled by another payment: this one must be refunded.
    /// The reservation itself is unchanged.
    RefundPayment,
}

impl ReservationStatus {
    /// Whether the reservation currently holds inventory.
    pub const fn holds_inventory(self) -> bool {
        matches!(self, Self::Reserved | Self::PaymentPending)
    }

    /// The buyer chooses how to pay. Repeating checkout is allowed (e.g. switching attestor).
    pub fn checkout(self) -> Result<Self, IllegalTransition> {
        match self {
            Self::Reserved | Self::PaymentPending => Ok(Self::PaymentPending),
            from => Err(IllegalTransition {
                from,
                action: "check out",
            }),
        }
    }

    /// The buyer gives up the reservation. Idempotent.
    pub fn cancel(self) -> Result<Self, IllegalTransition> {
        match self {
            Self::Reserved | Self::PaymentPending | Self::Cancelled => Ok(Self::Cancelled),
            from => Err(IllegalTransition {
                from,
                action: "cancel",
            }),
        }
    }

    /// The hold's time-to-live elapsed.
    pub fn expire(self) -> Result<Self, IllegalTransition> {
        match self {
            Self::Reserved | Self::PaymentPending => Ok(Self::Expired),
            from => Err(IllegalTransition {
                from,
                action: "expire",
            }),
        }
    }

    /// How to treat a newly attested payment.
    pub const fn settle(self) -> Settlement {
        match self {
            Self::Reserved | Self::PaymentPending => Settlement::Issue,
            Self::Expired | Self::Cancelled => Settlement::Reacquire,
            Self::Issued | Self::RefundRequired | Self::Refunded => Settlement::RefundPayment,
        }
    }

    /// The outcome of [`Settlement::Issue`] or of a successful [`Settlement::Reacquire`].
    pub fn issue(self) -> Result<Self, IllegalTransition> {
        match self {
            Self::Reserved | Self::PaymentPending | Self::Expired | Self::Cancelled => {
                Ok(Self::Issued)
            }
            from => Err(IllegalTransition {
                from,
                action: "issue",
            }),
        }
    }

    /// The outcome of a failed [`Settlement::Reacquire`].
    pub fn require_refund(self) -> Result<Self, IllegalTransition> {
        match self {
            Self::Expired | Self::Cancelled => Ok(Self::RefundRequired),
            from => Err(IllegalTransition {
                from,
                action: "require a refund for",
            }),
        }
    }

    /// The attestor confirmed it returned the money. Idempotent.
    pub fn refunded(self) -> Result<Self, IllegalTransition> {
        match self {
            Self::RefundRequired | Self::Refunded => Ok(Self::Refunded),
            from => Err(IllegalTransition {
                from,
                action: "mark as refunded",
            }),
        }
    }

    /// The status's name as stored and serialized.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Reserved => "reserved",
            Self::PaymentPending => "payment_pending",
            Self::Issued => "issued",
            Self::Expired => "expired",
            Self::Cancelled => "cancelled",
            Self::RefundRequired => "refund_required",
            Self::Refunded => "refunded",
        }
    }

    /// Every status.
    pub const ALL: [Self; 7] = [
        Self::Reserved,
        Self::PaymentPending,
        Self::Issued,
        Self::Expired,
        Self::Cancelled,
        Self::RefundRequired,
        Self::Refunded,
    ];
}

impl std::str::FromStr for ReservationStatus {
    type Err = ValidationError;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|status| status.as_str() == name)
            .ok_or(ValidationError::new(
                "status",
                "is not a known reservation status",
            ))
    }
}

/// Tickets of one type held by a reservation, at the price when they were reserved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ReservedItem {
    /// The ticket type.
    pub ticket_type_id: TicketTypeId,
    /// How many tickets.
    pub quantity: u32,
    /// Price of one ticket.
    pub unit_price: Money,
}

impl ReservedItem {
    /// What `items` cost together: `None` if there are none, they mix currencies or the sum
    /// overflows. The first failure ends the sum.
    pub fn total(items: &[Self]) -> Option<Money> {
        let mut lines = items
            .iter()
            .map(|item| item.unit_price.checked_mul(item.quantity));
        let first = lines.next()??;
        lines.try_fold(first, |sum, line| sum.checked_add(line?))
    }
}

/// A temporary hold on inventory for one buyer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Reservation {
    /// Identity of the reservation.
    pub id: ReservationId,
    /// The purchase request that produced it. Unique: a request reserves at most once.
    pub purchase_request_id: PurchaseRequestId,
    /// The buyer.
    pub account_id: AccountId,
    /// The sale bought from.
    pub sale_id: SaleId,
    /// The event the tickets admit to.
    pub event_id: EventId,
    /// What is held.
    pub items: Vec<ReservedItem>,
    /// What the buyer must pay.
    pub total: Money,
    /// Whether real money is involved.
    pub environment: Environment,
    /// Where the reservation stands.
    pub status: ReservationStatus,
    /// The attestor chosen at checkout.
    pub attestor_id: Option<AttestorId>,
    /// When an unpaid hold lapses.
    pub expires_at: Timestamp,
    /// When the reservation was made.
    pub created_at: Timestamp,
    /// When the status last changed.
    pub updated_at: Timestamp,
}

impl Reservation {
    /// Whether the hold has lapsed at `now` and should be expired.
    pub fn is_overdue(&self, now: Timestamp) -> bool {
        self.status.holds_inventory() && self.expires_at <= now
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::ReservationStatus::*;
    use super::*;

    #[test]
    fn the_happy_path() {
        let status = Reserved.checkout().unwrap();
        assert_eq!(status, PaymentPending);
        assert_eq!(status.settle(), Settlement::Issue);
        assert_eq!(status.issue().unwrap(), Issued);
    }

    #[test]
    fn late_payments_reacquire_or_require_a_refund() {
        let expired = PaymentPending.expire().unwrap();
        assert_eq!(expired.settle(), Settlement::Reacquire);
        assert_eq!(expired.issue().unwrap(), Issued);
        assert_eq!(expired.require_refund().unwrap(), RefundRequired);
        assert_eq!(RefundRequired.refunded().unwrap(), Refunded);
    }

    #[test]
    fn a_second_payment_is_refunded() {
        assert_eq!(Issued.settle(), Settlement::RefundPayment);
        assert_eq!(Refunded.settle(), Settlement::RefundPayment);
    }

    #[test]
    fn issued_tickets_cannot_be_cancelled_or_expired() {
        assert!(Issued.cancel().is_err());
        assert!(Issued.expire().is_err());
        assert!(Issued.checkout().is_err());
    }

    fn line(amount_minor: i64, currency: &str, quantity: u32) -> ReservedItem {
        ReservedItem {
            ticket_type_id: TicketTypeId::generate(),
            quantity,
            unit_price: Money::new(amount_minor, currency.parse().unwrap()).unwrap(),
        }
    }

    #[test]
    fn totals_stop_at_the_first_failure() {
        let total = ReservedItem::total(&[line(100, "JPY", 2), line(50, "JPY", 1)]).unwrap();
        assert_eq!(total.amount_minor, 250);
        // A failed line never restarts the sum with the lines after it.
        let mixed = [
            line(100, "JPY", 1),
            line(100, "CNY", 1),
            line(100, "CNY", 1),
        ];
        assert_eq!(ReservedItem::total(&mixed), None);
        let overflow = [
            line(i64::MAX, "JPY", 1),
            line(1, "JPY", 1),
            line(1, "JPY", 1),
        ];
        assert_eq!(ReservedItem::total(&overflow), None);
        assert_eq!(ReservedItem::total(&[line(i64::MAX, "JPY", 2)]), None);
        assert_eq!(ReservedItem::total(&[]), None);
    }

    #[test]
    fn statuses_round_trip_through_their_names() {
        for status in ReservationStatus::ALL {
            assert_eq!(
                status.as_str().parse::<ReservationStatus>().unwrap(),
                status
            );
        }
    }

    #[derive(Debug, Clone, Copy)]
    enum Step {
        Checkout,
        Cancel,
        Expire,
        Pay { stock_left: bool },
        Refunded,
    }

    fn step() -> impl Strategy<Value = Step> {
        prop_oneof![
            Just(Step::Checkout),
            Just(Step::Cancel),
            Just(Step::Expire),
            any::<bool>().prop_map(|stock_left| Step::Pay { stock_left }),
            Just(Step::Refunded),
        ]
    }

    /// Applies a step the way the application does: illegal transitions leave the state as is.
    fn apply(status: ReservationStatus, step: Step) -> ReservationStatus {
        let next = match step {
            Step::Checkout => status.checkout(),
            Step::Cancel => status.cancel(),
            Step::Expire => status.expire(),
            Step::Refunded => status.refunded(),
            Step::Pay { stock_left } => match status.settle() {
                Settlement::Issue => status.issue(),
                Settlement::Reacquire if stock_left => status.issue(),
                Settlement::Reacquire => status.require_refund(),
                Settlement::RefundPayment => Ok(status),
            },
        };
        next.unwrap_or(status)
    }

    proptest! {
        /// Whatever happens, once tickets are issued they stay issued, and a payment always
        /// ends in tickets or a refund.
        #[test]
        fn paid_reservations_end_in_tickets_or_refunds(steps in proptest::collection::vec(step(), 0..20)) {
            let mut status = Reserved;
            let mut paid = false;
            for step in steps {
                let before = status;
                status = apply(status, step);
                if before == Issued {
                    prop_assert_eq!(status, Issued);
                }
                if matches!(step, Step::Pay { .. }) {
                    paid = true;
                    prop_assert!(matches!(status, Issued | RefundRequired | Refunded));
                }
            }
            if paid {
                prop_assert!(!status.holds_inventory());
            }
        }
    }
}
