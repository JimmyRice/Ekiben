use std::collections::BTreeMap;

use kippu_domain::admission::AdmissionPolicy;
use kippu_domain::catalog::{EventStatus, Sale, TicketType};
use kippu_domain::payment::Environment;
use kippu_domain::{AttestorId, Money, Timestamp, ValidationError};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::service::{
    EventChanges, NewEvent, SaleChanges, SaleOffer, SaleSettings, TicketTypeAvailability,
    TicketTypeChanges, TicketTypeSettings,
};

/// The `version` a full update must carry; it is optional only so one body serves for
/// creating too.
pub(crate) fn required_version(version: Option<i64>) -> Result<i64, ValidationError> {
    version.ok_or_else(|| ValidationError::new("version", "is required when updating"))
}

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
    /// The event's page — HTML, Markdown, JSON for your own components, anything — stored as
    /// is, at most 256 KiB. Listings leave it out.
    #[serde(default)]
    pub content: String,
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
    /// The event's page — HTML, Markdown, JSON for your own components, anything — stored as
    /// is, at most 256 KiB. Listings leave it out.
    #[serde(default)]
    pub content: String,
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
    /// The event's page, replaced as a whole; at most 256 KiB.
    pub content: Option<String>,
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

impl From<CreateEventRequest> for NewEvent {
    fn from(request: CreateEventRequest) -> Self {
        Self {
            slug: request.slug,
            title: request.title,
            description: request.description,
            venue: request.venue,
            starts_at: request.starts_at,
            ends_at: request.ends_at,
            content: request.content,
        }
    }
}

impl From<UpdateEventRequest> for EventChanges {
    fn from(request: UpdateEventRequest) -> Self {
        Self {
            slug: Some(request.slug),
            title: Some(request.title),
            description: Some(request.description),
            venue: Some(request.venue),
            starts_at: Some(request.starts_at),
            ends_at: Some(request.ends_at),
            status: Some(request.status),
            content: Some(request.content),
        }
    }
}

impl From<EventPatch> for EventChanges {
    fn from(patch: EventPatch) -> Self {
        Self {
            slug: patch.slug,
            title: patch.title,
            description: patch.description,
            venue: patch.venue,
            starts_at: patch.starts_at,
            ends_at: patch.ends_at,
            status: patch.status,
            content: patch.content,
        }
    }
}

impl From<SaleRequest> for SaleSettings {
    fn from(request: SaleRequest) -> Self {
        Self {
            name: request.name,
            opens_at: request.opens_at,
            closes_at: request.closes_at,
            admission: request.admission,
            reservation_ttl_seconds: request.reservation_ttl_seconds,
            max_tickets_per_request: request.max_tickets_per_request,
            accepted_attestors: request.accepted_attestors,
            environment: request.environment,
        }
    }
}

impl From<SalePatch> for SaleChanges {
    fn from(patch: SalePatch) -> Self {
        Self {
            name: patch.name,
            opens_at: patch.opens_at,
            closes_at: patch.closes_at,
            admission: patch.admission,
            reservation_ttl_seconds: patch.reservation_ttl_seconds,
            max_tickets_per_request: patch.max_tickets_per_request,
            accepted_attestors: patch.accepted_attestors,
            environment: patch.environment,
        }
    }
}

impl From<TicketTypeRequest> for TicketTypeSettings {
    fn from(request: TicketTypeRequest) -> Self {
        Self {
            name: request.name,
            price: request.price,
            capacity: request.capacity,
            per_account_limit: request.per_account_limit,
            valid_from: request.valid_from,
            valid_until: request.valid_until,
            ticket_extensions: request.ticket_extensions,
        }
    }
}

impl From<TicketTypePatch> for TicketTypeChanges {
    fn from(patch: TicketTypePatch) -> Self {
        Self {
            name: patch.name,
            price: patch.price,
            capacity: patch.capacity,
            per_account_limit: patch.per_account_limit,
            valid_from: patch.valid_from,
            valid_until: patch.valid_until,
            ticket_extensions: patch.ticket_extensions,
        }
    }
}

impl From<SaleOffer> for SaleDetail {
    fn from(offer: SaleOffer) -> Self {
        Self {
            sale: offer.sale,
            ticket_types: offer
                .ticket_types
                .into_iter()
                .map(TicketTypeOffer::from)
                .collect(),
        }
    }
}

impl From<TicketTypeAvailability> for TicketTypeOffer {
    fn from(availability: TicketTypeAvailability) -> Self {
        Self {
            ticket_type: availability.ticket_type,
            available: availability.available,
        }
    }
}
