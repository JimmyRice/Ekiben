//! What is on sale: events, sales and ticket types.
//!
//! ```text
//! Organization ─< Event ─< Sale ─< TicketType ── Inventory
//! ```
//!
//! A sale is a window in which some ticket types are sold (early bird, general, on the day).
//! Each ticket type belongs to exactly one sale and has its own price and capacity.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::admission::AdmissionPolicy;
use crate::payment::Environment;
use crate::validation::{Slug, ValidationError, non_empty};
use crate::{AttestorId, EventId, Money, OrganizationId, SaleId, TicketTypeId, Timestamp};

/// Whether an event is visible to the public.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventStatus {
    /// Visible only to its organizers.
    Draft,
    /// Visible to everyone.
    Published,
    /// Called off. Still visible, nothing is sold.
    Cancelled,
}

/// A convention or other event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    /// Identity of the event.
    pub id: EventId,
    /// The organization running the event.
    pub organization_id: OrganizationId,
    /// URL-friendly name, unique across events.
    pub slug: Slug,
    /// Display title.
    pub title: String,
    /// Long description.
    pub description: String,
    /// Where it takes place.
    pub venue: String,
    /// When it opens.
    pub starts_at: Timestamp,
    /// When it closes.
    pub ends_at: Timestamp,
    /// Visibility.
    pub status: EventStatus,
    /// When the event was created.
    pub created_at: Timestamp,
    /// When the event was last changed.
    pub updated_at: Timestamp,
    /// Incremented on every change, for optimistic concurrency (`If-Match`).
    pub version: i64,
}

impl Event {
    /// Checks the invariants of an event's editable fields.
    pub fn validate(&self) -> Result<(), ValidationError> {
        non_empty("title", &self.title, 200)?;
        non_empty("venue", &self.venue, 200)?;
        if self.description.chars().count() > 20_000 {
            return Err(ValidationError::new("description", "is too long"));
        }
        if self.ends_at <= self.starts_at {
            return Err(ValidationError::new("ends_at", "must be after starts_at"));
        }
        Ok(())
    }

    /// Whether the public may see the event.
    pub const fn is_public(&self) -> bool {
        matches!(self.status, EventStatus::Published | EventStatus::Cancelled)
    }
}

/// A window in which some of an event's ticket types are sold.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sale {
    /// Identity of the sale.
    pub id: SaleId,
    /// The event the sale belongs to.
    pub event_id: EventId,
    /// Display name, e.g. "Early bird".
    pub name: String,
    /// When purchase requests start being accepted.
    pub opens_at: Timestamp,
    /// When purchase requests stop being accepted.
    pub closes_at: Timestamp,
    /// How buyers are let in when demand is high.
    pub admission: AdmissionPolicy,
    /// How long a reservation holds inventory before it expires unpaid.
    pub reservation_ttl_seconds: u32,
    /// Most tickets a single purchase request may ask for.
    pub max_tickets_per_request: u32,
    /// Attestors whose payment attestations this sale accepts.
    pub accepted_attestors: Vec<AttestorId>,
    /// Whether the sale takes real money. Sandbox attestors can only settle sandbox sales.
    pub environment: Environment,
    /// When the sale was created.
    pub created_at: Timestamp,
    /// Incremented on every change.
    pub version: i64,
}

impl Sale {
    /// Checks the invariants of a sale's editable fields.
    pub fn validate(&self) -> Result<(), ValidationError> {
        non_empty("name", &self.name, 200)?;
        if self.closes_at <= self.opens_at {
            return Err(ValidationError::new("closes_at", "must be after opens_at"));
        }
        if !(60..=86_400).contains(&self.reservation_ttl_seconds) {
            return Err(ValidationError::new(
                "reservation_ttl_seconds",
                "must be between 60 and 86400",
            ));
        }
        if !(1..=100).contains(&self.max_tickets_per_request) {
            return Err(ValidationError::new(
                "max_tickets_per_request",
                "must be between 1 and 100",
            ));
        }
        self.admission.validate()
    }

    /// Whether purchase requests are accepted at `now`.
    pub fn is_open_at(&self, now: Timestamp) -> bool {
        self.opens_at <= now && now < self.closes_at
    }

    /// Whether payments from `attestor` may settle reservations of this sale.
    pub fn accepts(&self, attestor: AttestorId) -> bool {
        self.accepted_attestors.contains(&attestor)
    }
}

/// A kind of ticket within a sale, with its own price, capacity and validity window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketType {
    /// Identity of the ticket type.
    pub id: TicketTypeId,
    /// The sale it is sold in.
    pub sale_id: SaleId,
    /// The event it admits to.
    pub event_id: EventId,
    /// Display name, e.g. "Day 1".
    pub name: String,
    /// Price of one ticket.
    pub price: Money,
    /// How many tickets of this type exist.
    pub capacity: u32,
    /// Most tickets of this type one account may hold.
    pub per_account_limit: u32,
    /// Issued tickets are valid from this instant (inclusive).
    pub valid_from: Timestamp,
    /// Issued tickets are valid until this instant (exclusive).
    pub valid_until: Timestamp,
    /// Extension claims (tags `0x80..=0xFF`) written into every ticket of this type,
    /// such as a hall or a day number. Gates read them with Kaisatsu.
    pub ticket_extensions: BTreeMap<u8, String>,
    /// When the ticket type was created.
    pub created_at: Timestamp,
    /// Incremented on every change.
    pub version: i64,
}

impl TicketType {
    /// Checks the invariants of a ticket type's editable fields.
    pub fn validate(&self) -> Result<(), ValidationError> {
        non_empty("name", &self.name, 200)?;
        if self.capacity == 0 {
            return Err(ValidationError::new("capacity", "must be positive"));
        }
        if self.per_account_limit == 0 {
            return Err(ValidationError::new(
                "per_account_limit",
                "must be positive",
            ));
        }
        if self.valid_until <= self.valid_from {
            return Err(ValidationError::new(
                "valid_until",
                "must be after valid_from",
            ));
        }
        for (&tag, value) in &self.ticket_extensions {
            if tag < 0x80 {
                return Err(ValidationError::new(
                    "ticket_extensions",
                    "tags must be between 128 and 255",
                ));
            }
            if value.len() > 512 {
                return Err(ValidationError::new(
                    "ticket_extensions",
                    "values must be at most 512 bytes",
                ));
            }
        }
        Ok(())
    }
}

/// Stock of one ticket type. `held + sold` never exceeds `capacity`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inventory {
    /// The ticket type this stock is for.
    pub ticket_type_id: TicketTypeId,
    /// Total tickets.
    pub capacity: u32,
    /// Tickets held by unpaid reservations.
    pub held: u32,
    /// Tickets issued.
    pub sold: u32,
}

impl Inventory {
    /// Tickets that can still be reserved.
    pub const fn available(&self) -> u32 {
        self.capacity
            .saturating_sub(self.held.saturating_add(self.sold))
    }
}
