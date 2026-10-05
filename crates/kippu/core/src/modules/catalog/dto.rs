use std::collections::BTreeMap;

use kippu_domain::admission::AdmissionPolicy;
use kippu_domain::catalog::{EventStatus, Sale, TicketType};
use kippu_domain::payment::Environment;
use kippu_domain::{AttestorId, Money, Timestamp};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// A new event. It starts as a draft.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateEventRequest {
    /// URL-friendly unique name.
    pub slug: String,
    /// Display title.
    pub title: String,
    /// Long description.
    #[serde(default)]
    pub description: String,
    /// Where it takes place.
    pub venue: String,
    /// When it opens.
    pub starts_at: Timestamp,
    /// When it closes.
    pub ends_at: Timestamp,
}

/// An event's new state. `version` must be the version you last read.
#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateEventRequest {
    /// The version you are editing; a mismatch fails with 412.
    pub version: i64,
    /// URL-friendly unique name.
    pub slug: String,
    /// Display title.
    pub title: String,
    /// Long description.
    #[serde(default)]
    pub description: String,
    /// Where it takes place.
    pub venue: String,
    /// When it opens.
    pub starts_at: Timestamp,
    /// When it closes.
    pub ends_at: Timestamp,
    /// `draft`, `published` or `cancelled`.
    pub status: EventStatus,
}

/// A sale's settings, for creating or (with `version`) updating it.
#[derive(Debug, Deserialize, ToSchema)]
pub struct SaleRequest {
    /// Required when updating: the version you are editing.
    pub version: Option<i64>,
    /// Display name, e.g. "Early bird".
    pub name: String,
    /// When purchase requests start being accepted.
    pub opens_at: Timestamp,
    /// When purchase requests stop being accepted.
    pub closes_at: Timestamp,
    /// `{"mode": "open"}` or `{"mode": "waiting_room", "admit_per_tick": 100, "max_backlog": 500}`.
    #[serde(default = "open_admission")]
    pub admission: AdmissionPolicy,
    /// How long an unpaid reservation holds tickets (default 600).
    #[serde(default = "default_ttl")]
    pub reservation_ttl_seconds: u32,
    /// Most tickets per purchase request (default 4).
    #[serde(default = "default_max_tickets")]
    pub max_tickets_per_request: u32,
    /// Attestors whose payments settle this sale's reservations.
    #[serde(default)]
    pub accepted_attestors: Vec<AttestorId>,
    /// `live` (default) or `sandbox`.
    #[serde(default = "live")]
    pub environment: Environment,
}

const fn open_admission() -> AdmissionPolicy {
    AdmissionPolicy::Open
}

const fn default_ttl() -> u32 {
    600
}

const fn default_max_tickets() -> u32 {
    4
}

const fn live() -> Environment {
    Environment::Live
}

/// A ticket type's settings, for creating or (with `version`) updating it.
#[derive(Debug, Deserialize, ToSchema)]
pub struct TicketTypeRequest {
    /// Required when updating: the version you are editing.
    pub version: Option<i64>,
    /// Display name, e.g. "Day 1".
    pub name: String,
    /// Price of one ticket.
    pub price: Money,
    /// How many tickets exist. Can be raised, or lowered down to what is held and sold.
    pub capacity: u32,
    /// Most tickets of this type one account may hold (default 4).
    #[serde(default = "default_max_tickets")]
    pub per_account_limit: u32,
    /// Tickets are valid from (inclusive).
    pub valid_from: Timestamp,
    /// Tickets are valid until (exclusive).
    pub valid_until: Timestamp,
    /// Extension claims written into every ticket, keyed by tag (128-255).
    #[serde(default)]
    pub ticket_extensions: BTreeMap<u8, String>,
}

/// Changes to an event: only the fields to change, plus the `version` you last read.
///
/// Absent fields keep their value. Unknown fields are rejected, so a typo cannot be silently
/// ignored.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct EventPatch {
    /// The version you are editing; a mismatch fails with 412.
    pub version: i64,
    /// URL-friendly unique name.
    pub slug: Option<String>,
    /// Display title.
    pub title: Option<String>,
    /// Long description.
    pub description: Option<String>,
    /// Where it takes place.
    pub venue: Option<String>,
    /// When it opens.
    pub starts_at: Option<Timestamp>,
    /// When it closes.
    pub ends_at: Option<Timestamp>,
    /// `draft`, `published` or `cancelled`.
    pub status: Option<EventStatus>,
}

/// Changes to a sale: only the fields to change, plus the `version` you last read.
///
/// Absent fields keep their value; unknown fields are rejected.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SalePatch {
    /// The version you are editing; a mismatch fails with 412.
    pub version: i64,
    /// Display name.
    pub name: Option<String>,
    /// When purchase requests start being accepted.
    pub opens_at: Option<Timestamp>,
    /// When purchase requests stop being accepted.
    pub closes_at: Option<Timestamp>,
    /// The admission policy, replaced as a whole.
    pub admission: Option<AdmissionPolicy>,
    /// How long an unpaid reservation holds tickets.
    pub reservation_ttl_seconds: Option<u32>,
    /// Most tickets per purchase request.
    pub max_tickets_per_request: Option<u32>,
    /// Attestors whose payments settle this sale's reservations, replaced as a whole.
    pub accepted_attestors: Option<Vec<AttestorId>>,
    /// `live` or `sandbox`.
    pub environment: Option<Environment>,
}

/// Changes to a ticket type: only the fields to change, plus the `version` you last read.
///
/// Absent fields keep their value; unknown fields are rejected.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TicketTypePatch {
    /// The version you are editing; a mismatch fails with 412.
    pub version: i64,
    /// Display name.
    pub name: Option<String>,
    /// Price of one ticket.
    pub price: Option<Money>,
    /// How many tickets exist. Can be raised, or lowered down to what is held and sold.
    pub capacity: Option<u32>,
    /// Most tickets of this type one account may hold.
    pub per_account_limit: Option<u32>,
    /// Tickets are valid from (inclusive).
    pub valid_from: Option<Timestamp>,
    /// Tickets are valid until (exclusive).
    pub valid_until: Option<Timestamp>,
    /// Extension claims written into every ticket, replaced as a whole.
    pub ticket_extensions: Option<BTreeMap<u8, String>>,
}

/// A sale with its ticket types and current availability.
#[derive(Debug, Serialize, ToSchema)]
pub struct SaleDetail {
    /// The sale.
    #[serde(flatten)]
    pub sale: Sale,
    /// What it sells.
    pub ticket_types: Vec<TicketTypeOffer>,
}

/// A ticket type and how many tickets are left.
#[derive(Debug, Serialize, ToSchema)]
pub struct TicketTypeOffer {
    /// The ticket type.
    #[serde(flatten)]
    pub ticket_type: TicketType,
    /// Tickets that can still be reserved right now.
    pub available: u32,
}
