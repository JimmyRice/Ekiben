//! Purchase requests: the asynchronous "remote future" a buyer polls.
//!
//! Submitting a purchase only records the intent. A worker later processes it at a steady
//! rate and either reserves inventory or rejects it. Because the request id is *derived* from
//! the buyer, the sale and the idempotency key, retrying a submission — on any server, any
//! number of times — always names the same request.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::validation::{IdempotencyKey, ValidationError};
use crate::{AccountId, PurchaseRequestId, ReservationId, SaleId, TicketTypeId, Timestamp};

/// Most distinct ticket types in one request.
pub const MAX_LINE_ITEMS: usize = 10;

impl PurchaseRequestId {
    /// The id of the purchase request `account` submits to `sale` with `key`.
    ///
    /// A `UUIDv8` whose bits come from SHA-256 over the three inputs, so it is the same on every
    /// server and for every retry.
    pub fn derive(account: AccountId, sale: SaleId, key: &IdempotencyKey) -> Self {
        let digest = Sha256::new()
            .chain_update(b"kippu/purchase-request/v1\0")
            .chain_update(account.as_uuid().as_bytes())
            .chain_update(sale.as_uuid().as_bytes())
            .chain_update(key.as_str().as_bytes())
            .finalize();
        let mut bytes = [0; 16];
        bytes.copy_from_slice(digest.get(..16).unwrap_or(&[0; 16]));
        Self::from_uuid(Uuid::new_v8(bytes))
    }
}

/// One line of a basket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LineItem {
    /// What to buy.
    pub ticket_type_id: TicketTypeId,
    /// How many.
    pub quantity: u32,
}

/// The tickets a purchase request asks for, in canonical form: sorted by ticket type, one line
/// per type, every quantity positive.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "Vec<LineItem>", into = "Vec<LineItem>")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema), schema(value_type = Vec<LineItem>))]
pub struct Basket(Vec<LineItem>);

impl Basket {
    /// Normalizes and validates line items. Lines for the same ticket type are merged.
    pub fn new(mut items: Vec<LineItem>) -> Result<Self, ValidationError> {
        if items.iter().any(|item| item.quantity == 0) {
            return Err(ValidationError::new("items", "quantities must be positive"));
        }
        items.sort_by_key(|item| item.ticket_type_id);
        let mut merged: Vec<LineItem> = Vec::with_capacity(items.len());
        for item in items {
            match merged.last_mut() {
                Some(last) if last.ticket_type_id == item.ticket_type_id => {
                    last.quantity = last
                        .quantity
                        .checked_add(item.quantity)
                        .ok_or(ValidationError::new("items", "quantity is too large"))?;
                }
                _ => merged.push(item),
            }
        }
        if merged.is_empty() {
            return Err(ValidationError::new("items", "must not be empty"));
        }
        if merged.len() > MAX_LINE_ITEMS {
            return Err(ValidationError::new("items", "has too many ticket types"));
        }
        Ok(Self(merged))
    }

    /// The line items, sorted by ticket type.
    pub fn items(&self) -> &[LineItem] {
        &self.0
    }

    /// Total number of tickets.
    pub fn total_quantity(&self) -> u64 {
        self.0.iter().map(|item| u64::from(item.quantity)).sum()
    }

    /// A digest of the basket, used to tell a genuine retry from a different request that
    /// reuses an idempotency key.
    pub fn fingerprint(&self) -> String {
        let mut hasher = Sha256::new();
        for item in &self.0 {
            hasher.update(item.ticket_type_id.as_uuid().as_bytes());
            hasher.update(item.quantity.to_be_bytes());
        }
        hex::encode(hasher.finalize())
    }
}

impl TryFrom<Vec<LineItem>> for Basket {
    type Error = ValidationError;

    fn try_from(items: Vec<LineItem>) -> Result<Self, Self::Error> {
        Self::new(items)
    }
}

impl From<Basket> for Vec<LineItem> {
    fn from(basket: Basket) -> Self {
        basket.0
    }
}

/// Where a purchase request stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum PurchaseStatus {
    /// Waiting for a worker.
    Queued,
    /// Inventory is held; pay before the reservation expires.
    Reserved {
        /// The reservation holding the tickets.
        reservation_id: ReservationId,
    },
    /// The request could not be fulfilled.
    Rejected {
        /// Why.
        reason: RejectionReason,
    },
}

/// Why a purchase request was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum RejectionReason {
    /// Not enough tickets left.
    SoldOut,
    /// The buyer would exceed a per-account limit.
    LimitExceeded,
    /// The sale was not open when the request was processed.
    SaleClosed,
    /// The basket names a ticket type that is not part of the sale.
    UnknownTicketType,
}

impl RejectionReason {
    /// The reason's name as stored and serialized.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SoldOut => "sold_out",
            Self::LimitExceeded => "limit_exceeded",
            Self::SaleClosed => "sale_closed",
            Self::UnknownTicketType => "unknown_ticket_type",
        }
    }
}

impl std::str::FromStr for RejectionReason {
    type Err = ValidationError;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        [
            Self::SoldOut,
            Self::LimitExceeded,
            Self::SaleClosed,
            Self::UnknownTicketType,
        ]
        .into_iter()
        .find(|reason| reason.as_str() == name)
        .ok_or(ValidationError::new(
            "reason",
            "is not a known rejection reason",
        ))
    }
}

/// A buyer's request to purchase a basket of tickets from a sale.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PurchaseRequest {
    /// Derived from account, sale and idempotency key.
    pub id: PurchaseRequestId,
    /// The buyer.
    pub account_id: AccountId,
    /// The sale bought from.
    pub sale_id: SaleId,
    /// What is requested.
    pub basket: Basket,
    /// Where the request stands. Flattened: `{"status": "rejected", "reason": "sold_out"}`.
    #[serde(flatten)]
    pub status: PurchaseStatus,
    /// When the request was first accepted.
    pub created_at: Timestamp,
    /// When the status last changed.
    pub updated_at: Timestamp,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(byte: u8, quantity: u32) -> LineItem {
        LineItem {
            ticket_type_id: TicketTypeId::from_uuid(Uuid::from_bytes([byte; 16])),
            quantity,
        }
    }

    #[test]
    fn request_ids_are_derived_deterministically() {
        let account = AccountId::from_uuid(Uuid::from_bytes([1; 16]));
        let sale = SaleId::from_uuid(Uuid::from_bytes([2; 16]));
        let key = IdempotencyKey::new("abc").unwrap();
        let other = IdempotencyKey::new("abd").unwrap();

        let id = PurchaseRequestId::derive(account, sale, &key);
        assert_eq!(id, PurchaseRequestId::derive(account, sale, &key));
        assert_ne!(id, PurchaseRequestId::derive(account, sale, &other));
        assert_eq!(id.as_uuid().get_version_num(), 8);
    }

    #[test]
    fn baskets_are_canonical() {
        let a = Basket::new(vec![line(2, 1), line(1, 1), line(2, 2)]).unwrap();
        let b = Basket::new(vec![line(1, 1), line(2, 3)]).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.fingerprint(), b.fingerprint());
        assert_eq!(a.total_quantity(), 4);
    }

    #[test]
    fn baskets_reject_empty_zero_and_oversized_requests() {
        assert!(Basket::new(vec![]).is_err());
        assert!(Basket::new(vec![line(1, 0)]).is_err());
        assert!(Basket::new((0..11).map(|i| line(i, 1)).collect()).is_err());
        assert!(Basket::new(vec![line(1, u32::MAX), line(1, 1)]).is_err());
    }

    #[test]
    fn statuses_serialize_with_a_tag() {
        let json = serde_json::to_string(&PurchaseStatus::Rejected {
            reason: RejectionReason::SoldOut,
        })
        .unwrap();
        assert_eq!(json, r#"{"status":"rejected","reason":"sold_out"}"#);
    }
}
