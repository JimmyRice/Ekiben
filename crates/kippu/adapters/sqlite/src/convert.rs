//! Conversions between database rows and domain types.
//!
//! Each table has a `*Row` struct that mirrors its columns exactly, derived with `FromRow`,
//! and a `TryFrom` into the domain type. Anything that does not parse is corrupt data and
//! surfaces as a backend error.

use std::collections::BTreeMap;
use std::str::FromStr;

use kippu_domain::account::{Account, Identity, Organization, Role};
use kippu_domain::catalog::{Event, EventStatus, Inventory, Sale, TicketType};
use kippu_domain::payment::{
    Attestor, AttestorKey, Environment, PaymentAttestation, PaymentDisposition,
};
use kippu_domain::purchase::{Basket, PurchaseRequest, PurchaseStatus, RejectionReason};
use kippu_domain::reservation::{Reservation, ReservationStatus};
use kippu_domain::ticket::{Ticket, TicketStatus};
use kippu_domain::validation::{Email, ProviderName, Slug, Subject};
use kippu_domain::webhook::Webhook;
use kippu_domain::{Currency, Money, Timestamp};
use kippu_store::StoreError;
use serde::Serialize;
use serde::de::DeserializeOwned;
use uuid::Uuid;

/// Converts a domain instant to its column value.
pub(crate) fn micros(instant: Timestamp) -> i64 {
    instant.unix_micros()
}

/// Converts a column value to a domain instant.
pub(crate) fn instant(micros: i64) -> Timestamp {
    Timestamp::from_unix_micros(micros)
}

/// Serializes a value for a JSON column.
pub(crate) fn json<T: Serialize>(value: &T) -> Result<String, StoreError> {
    serde_json::to_string(value).map_err(StoreError::backend)
}

/// Parses a JSON column.
fn from_json<T: DeserializeOwned>(text: &str) -> Result<T, StoreError> {
    serde_json::from_str(text).map_err(StoreError::backend)
}

/// Parses a text column into a domain value.
fn parse<T>(text: &str) -> Result<T, StoreError>
where
    T: FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    text.parse().map_err(StoreError::backend)
}

fn money(amount_minor: i64, currency: &str) -> Result<Money, StoreError> {
    Money::new(amount_minor, parse::<Currency>(currency)?).map_err(StoreError::backend)
}

/// Encodes a ticket type's extension claims, whose JSON object keys must be strings.
pub(crate) fn extensions_json(extensions: &BTreeMap<u8, String>) -> Result<String, StoreError> {
    json(
        &extensions
            .iter()
            .map(|(tag, value)| (tag.to_string(), value))
            .collect::<BTreeMap<_, _>>(),
    )
}

fn extensions_from_json(text: &str) -> Result<BTreeMap<u8, String>, StoreError> {
    from_json::<BTreeMap<String, String>>(text)?
        .into_iter()
        .map(|(tag, value)| Ok((parse::<u8>(&tag)?, value)))
        .collect()
}

#[derive(sqlx::FromRow)]
pub(crate) struct AccountRow {
    id: Uuid,
    email: Option<String>,
    display_name: String,
    role: String,
    created_at: i64,
}

impl TryFrom<AccountRow> for Account {
    type Error = StoreError;

    fn try_from(row: AccountRow) -> Result<Self, StoreError> {
        Ok(Self {
            id: row.id.into(),
            email: row
                .email
                .map(Email::new)
                .transpose()
                .map_err(StoreError::backend)?,
            display_name: row.display_name,
            role: parse::<Role>(&row.role)?,
            created_at: instant(row.created_at),
        })
    }
}

#[derive(sqlx::FromRow)]
pub(crate) struct CredentialsRow {
    #[sqlx(flatten)]
    pub(crate) account: AccountRow,
    pub(crate) password_hash: Option<String>,
}

#[derive(sqlx::FromRow)]
pub(crate) struct IdentityRow {
    provider: String,
    subject: String,
    account_id: Uuid,
    created_at: i64,
}

impl TryFrom<IdentityRow> for Identity {
    type Error = StoreError;

    fn try_from(row: IdentityRow) -> Result<Self, StoreError> {
        Ok(Self {
            provider: ProviderName::new(row.provider).map_err(StoreError::backend)?,
            subject: Subject::new(row.subject).map_err(StoreError::backend)?,
            account_id: row.account_id.into(),
            created_at: instant(row.created_at),
        })
    }
}

#[derive(sqlx::FromRow)]
pub(crate) struct OrganizationRow {
    id: Uuid,
    slug: String,
    name: String,
    created_at: i64,
}

impl TryFrom<OrganizationRow> for Organization {
    type Error = StoreError;

    fn try_from(row: OrganizationRow) -> Result<Self, StoreError> {
        Ok(Self {
            id: row.id.into(),
            slug: Slug::new(row.slug).map_err(StoreError::backend)?,
            name: row.name,
            created_at: instant(row.created_at),
        })
    }
}

pub(crate) fn event_status(status: EventStatus) -> &'static str {
    match status {
        EventStatus::Draft => "draft",
        EventStatus::Published => "published",
        EventStatus::Cancelled => "cancelled",
    }
}

#[derive(sqlx::FromRow)]
pub(crate) struct EventRow {
    id: Uuid,
    organization_id: Uuid,
    slug: String,
    title: String,
    description: String,
    venue: String,
    starts_at: i64,
    ends_at: i64,
    status: String,
    created_at: i64,
    updated_at: i64,
    version: i64,
}

impl TryFrom<EventRow> for Event {
    type Error = StoreError;

    fn try_from(row: EventRow) -> Result<Self, StoreError> {
        let status = match row.status.as_str() {
            "draft" => EventStatus::Draft,
            "published" => EventStatus::Published,
            "cancelled" => EventStatus::Cancelled,
            other => {
                return Err(StoreError::backend(format!(
                    "unknown event status {other:?}"
                )));
            }
        };
        Ok(Self {
            id: row.id.into(),
            organization_id: row.organization_id.into(),
            slug: Slug::new(row.slug).map_err(StoreError::backend)?,
            title: row.title,
            description: row.description,
            venue: row.venue,
            starts_at: instant(row.starts_at),
            ends_at: instant(row.ends_at),
            status,
            created_at: instant(row.created_at),
            updated_at: instant(row.updated_at),
            version: row.version,
        })
    }
}

#[derive(sqlx::FromRow)]
pub(crate) struct SaleRow {
    id: Uuid,
    event_id: Uuid,
    name: String,
    opens_at: i64,
    closes_at: i64,
    admission: String,
    reservation_ttl_seconds: u32,
    max_tickets_per_request: u32,
    accepted_attestors: String,
    environment: String,
    created_at: i64,
    version: i64,
}

impl TryFrom<SaleRow> for Sale {
    type Error = StoreError;

    fn try_from(row: SaleRow) -> Result<Self, StoreError> {
        Ok(Self {
            id: row.id.into(),
            event_id: row.event_id.into(),
            name: row.name,
            opens_at: instant(row.opens_at),
            closes_at: instant(row.closes_at),
            admission: from_json(&row.admission)?,
            reservation_ttl_seconds: row.reservation_ttl_seconds,
            max_tickets_per_request: row.max_tickets_per_request,
            accepted_attestors: from_json(&row.accepted_attestors)?,
            environment: parse::<Environment>(&row.environment)?,
            created_at: instant(row.created_at),
            version: row.version,
        })
    }
}

#[derive(sqlx::FromRow)]
pub(crate) struct TicketTypeRow {
    id: Uuid,
    sale_id: Uuid,
    event_id: Uuid,
    name: String,
    price_minor: i64,
    currency: String,
    capacity: u32,
    per_account_limit: u32,
    valid_from: i64,
    valid_until: i64,
    ticket_extensions: String,
    created_at: i64,
    version: i64,
}

impl TryFrom<TicketTypeRow> for TicketType {
    type Error = StoreError;

    fn try_from(row: TicketTypeRow) -> Result<Self, StoreError> {
        Ok(Self {
            id: row.id.into(),
            sale_id: row.sale_id.into(),
            event_id: row.event_id.into(),
            name: row.name,
            price: money(row.price_minor, &row.currency)?,
            capacity: row.capacity,
            per_account_limit: row.per_account_limit,
            valid_from: instant(row.valid_from),
            valid_until: instant(row.valid_until),
            ticket_extensions: extensions_from_json(&row.ticket_extensions)?,
            created_at: instant(row.created_at),
            version: row.version,
        })
    }
}

#[derive(sqlx::FromRow)]
pub(crate) struct InventoryRow {
    ticket_type_id: Uuid,
    capacity: u32,
    held: u32,
    sold: u32,
}

impl From<InventoryRow> for Inventory {
    fn from(row: InventoryRow) -> Self {
        Self {
            ticket_type_id: row.ticket_type_id.into(),
            capacity: row.capacity,
            held: row.held,
            sold: row.sold,
        }
    }
}

/// The columns a purchase status is stored in.
pub(crate) struct PurchaseStatusColumns {
    pub(crate) status: &'static str,
    pub(crate) reservation_id: Option<Uuid>,
    pub(crate) rejection_reason: Option<&'static str>,
}

pub(crate) fn purchase_status(status: PurchaseStatus) -> PurchaseStatusColumns {
    match status {
        PurchaseStatus::Queued => PurchaseStatusColumns {
            status: "queued",
            reservation_id: None,
            rejection_reason: None,
        },
        PurchaseStatus::Reserved { reservation_id } => PurchaseStatusColumns {
            status: "reserved",
            reservation_id: Some(reservation_id.as_uuid()),
            rejection_reason: None,
        },
        PurchaseStatus::Rejected { reason } => PurchaseStatusColumns {
            status: "rejected",
            reservation_id: None,
            rejection_reason: Some(reason.as_str()),
        },
    }
}

#[derive(sqlx::FromRow)]
pub(crate) struct PurchaseRequestRow {
    id: Uuid,
    account_id: Uuid,
    sale_id: Uuid,
    basket: String,
    status: String,
    reservation_id: Option<Uuid>,
    rejection_reason: Option<String>,
    created_at: i64,
    updated_at: i64,
}

impl TryFrom<PurchaseRequestRow> for PurchaseRequest {
    type Error = StoreError;

    fn try_from(row: PurchaseRequestRow) -> Result<Self, StoreError> {
        let status = match (
            row.status.as_str(),
            row.reservation_id,
            row.rejection_reason,
        ) {
            ("queued", _, _) => PurchaseStatus::Queued,
            ("reserved", Some(reservation_id), _) => PurchaseStatus::Reserved {
                reservation_id: reservation_id.into(),
            },
            ("rejected", _, Some(reason)) => PurchaseStatus::Rejected {
                reason: parse::<RejectionReason>(&reason)?,
            },
            (other, _, _) => {
                return Err(StoreError::backend(format!(
                    "inconsistent purchase status {other:?}"
                )));
            }
        };
        Ok(Self {
            id: row.id.into(),
            account_id: row.account_id.into(),
            sale_id: row.sale_id.into(),
            basket: from_json::<Basket>(&row.basket)?,
            status,
            created_at: instant(row.created_at),
            updated_at: instant(row.updated_at),
        })
    }
}

#[derive(sqlx::FromRow)]
pub(crate) struct ReservationRow {
    id: Uuid,
    purchase_request_id: Uuid,
    account_id: Uuid,
    sale_id: Uuid,
    event_id: Uuid,
    items: String,
    total_minor: i64,
    currency: String,
    environment: String,
    status: String,
    attestor_id: Option<Uuid>,
    expires_at: i64,
    created_at: i64,
    updated_at: i64,
}

impl TryFrom<ReservationRow> for Reservation {
    type Error = StoreError;

    fn try_from(row: ReservationRow) -> Result<Self, StoreError> {
        Ok(Self {
            id: row.id.into(),
            purchase_request_id: row.purchase_request_id.into(),
            account_id: row.account_id.into(),
            sale_id: row.sale_id.into(),
            event_id: row.event_id.into(),
            items: from_json(&row.items)?,
            total: money(row.total_minor, &row.currency)?,
            environment: parse::<Environment>(&row.environment)?,
            status: parse::<ReservationStatus>(&row.status)?,
            attestor_id: row.attestor_id.map(Into::into),
            expires_at: instant(row.expires_at),
            created_at: instant(row.created_at),
            updated_at: instant(row.updated_at),
        })
    }
}

#[derive(sqlx::FromRow)]
pub(crate) struct AttestorRow {
    id: Uuid,
    name: String,
    environment: String,
    revoked: bool,
    created_at: i64,
}

impl TryFrom<AttestorRow> for Attestor {
    type Error = StoreError;

    fn try_from(row: AttestorRow) -> Result<Self, StoreError> {
        Ok(Self {
            id: row.id.into(),
            name: row.name,
            environment: parse::<Environment>(&row.environment)?,
            revoked: row.revoked,
            created_at: instant(row.created_at),
        })
    }
}

#[derive(sqlx::FromRow)]
pub(crate) struct AttestorKeyRow {
    attestor_id: Uuid,
    key_id: String,
    public_key: Vec<u8>,
    revoked: bool,
    created_at: i64,
}

impl TryFrom<AttestorKeyRow> for AttestorKey {
    type Error = StoreError;

    fn try_from(row: AttestorKeyRow) -> Result<Self, StoreError> {
        Ok(Self {
            attestor_id: row.attestor_id.into(),
            key_id: row.key_id,
            public_key: row
                .public_key
                .try_into()
                .map_err(|_| StoreError::backend("attestor public key is not 32 bytes"))?,
            revoked: row.revoked,
            created_at: instant(row.created_at),
        })
    }
}

#[derive(sqlx::FromRow)]
pub(crate) struct AttestationRow {
    attestor_id: Uuid,
    attestation_id: String,
    reservation_id: Uuid,
    amount_minor: i64,
    currency: String,
    occurred_at: i64,
    received_at: i64,
    disposition: String,
}

impl TryFrom<AttestationRow> for PaymentAttestation {
    type Error = StoreError;

    fn try_from(row: AttestationRow) -> Result<Self, StoreError> {
        Ok(Self {
            attestor_id: row.attestor_id.into(),
            attestation_id: row.attestation_id,
            reservation_id: row.reservation_id.into(),
            amount: money(row.amount_minor, &row.currency)?,
            occurred_at: instant(row.occurred_at),
            received_at: instant(row.received_at),
            disposition: parse::<PaymentDisposition>(&row.disposition)?,
        })
    }
}

pub(crate) fn ticket_status(status: TicketStatus) -> &'static str {
    match status {
        TicketStatus::Valid => "valid",
        TicketStatus::Revoked => "revoked",
    }
}

#[derive(sqlx::FromRow)]
pub(crate) struct TicketRow {
    id: Uuid,
    reservation_id: Uuid,
    account_id: Uuid,
    event_id: Uuid,
    ticket_type_id: Uuid,
    valid_from: i64,
    valid_until: i64,
    issued_at: i64,
    status: String,
    encoded: Vec<u8>,
}

impl TryFrom<TicketRow> for Ticket {
    type Error = StoreError;

    fn try_from(row: TicketRow) -> Result<Self, StoreError> {
        let status = match row.status.as_str() {
            "valid" => TicketStatus::Valid,
            "revoked" => TicketStatus::Revoked,
            other => {
                return Err(StoreError::backend(format!(
                    "unknown ticket status {other:?}"
                )));
            }
        };
        Ok(Self {
            id: row.id.into(),
            reservation_id: row.reservation_id.into(),
            account_id: row.account_id.into(),
            event_id: row.event_id.into(),
            ticket_type_id: row.ticket_type_id.into(),
            valid_from: instant(row.valid_from),
            valid_until: instant(row.valid_until),
            issued_at: instant(row.issued_at),
            status,
            encoded: row.encoded,
        })
    }
}

#[derive(sqlx::FromRow)]
pub(crate) struct WebhookRow {
    id: Uuid,
    organization_id: Option<Uuid>,
    url: String,
    topics: String,
    active: bool,
    delivered_through: i64,
    failures: u32,
    last_error: Option<String>,
    next_attempt_at: i64,
    created_at: i64,
    version: i64,
}

impl TryFrom<WebhookRow> for Webhook {
    type Error = StoreError;

    fn try_from(row: WebhookRow) -> Result<Self, StoreError> {
        Ok(Self {
            id: row.id.into(),
            organization_id: row.organization_id.map(Into::into),
            url: row.url,
            topics: from_json(&row.topics)?,
            active: row.active,
            delivered_through: row.delivered_through,
            failures: row.failures,
            last_error: row.last_error,
            next_attempt_at: instant(row.next_attempt_at),
            created_at: instant(row.created_at),
            version: row.version,
        })
    }
}

/// Converts every row, failing on the first that does not parse.
pub(crate) fn all<R, T>(rows: Vec<R>) -> Result<Vec<T>, StoreError>
where
    T: TryFrom<R, Error = StoreError>,
{
    rows.into_iter().map(T::try_from).collect()
}

/// Converts an optional row.
pub(crate) fn optional<R, T>(row: Option<R>) -> Result<Option<T>, StoreError>
where
    T: TryFrom<R, Error = StoreError>,
{
    row.map(T::try_from).transpose()
}
