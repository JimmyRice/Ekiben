//! Denials: who and what must not get in, whatever their ticket says.
//!
//! Organizers keep deny lists, for one event or for every event of their organization. An
//! entry names a ticket (lost, stolen, sold on against the rules) or an account (a person who
//! may not attend). Entries can be added, annotated and lifted at any time.
//!
//! Gates verify tickets offline and see only what a ticket carries — never who holds it — so
//! what reaches them is always a set of ticket ids: the event's [`DeniedTicket`]s. It joins the
//! tickets denied directly, every ticket of a denied account, and tickets revoked by a refund.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::validation::ValidationError;
use crate::{AccountId, DenialId, EventId, OrganizationId, TicketId, TicketTypeId, Timestamp};

/// Longest note an organizer may keep with a denial.
pub const MAX_NOTE_CHARS: usize = 1_000;

/// What a denial refuses entry to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum DenialSubject {
    /// One ticket, whoever presents it.
    Ticket(TicketId),
    /// Every ticket an account holds for the events the denial covers, including tickets
    /// issued later. The account also cannot buy tickets for those events.
    Account(AccountId),
}

impl DenialSubject {
    /// The subject's kind as stored and serialized.
    pub const fn kind(self) -> &'static str {
        match self {
            Self::Ticket(_) => "ticket",
            Self::Account(_) => "account",
        }
    }

    /// The id of the ticket or account.
    pub const fn id(self) -> Uuid {
        match self {
            Self::Ticket(id) => id.as_uuid(),
            Self::Account(id) => id.as_uuid(),
        }
    }

    /// Rebuilds a subject from its stored kind and id.
    pub fn from_parts(kind: &str, id: Uuid) -> Result<Self, ValidationError> {
        match kind {
            "ticket" => Ok(Self::Ticket(id.into())),
            "account" => Ok(Self::Account(id.into())),
            _ => Err(ValidationError::new(
                "subject.kind",
                "must be ticket or account",
            )),
        }
    }
}

impl DenialId {
    /// The id of the denial of `subject` for `event` (or, when `None`, every event of
    /// `organization`). Denying the same subject in the same place twice names the same
    /// denial, so a retried request changes nothing.
    pub fn derive(
        organization: OrganizationId,
        event: Option<EventId>,
        subject: DenialSubject,
    ) -> Self {
        let digest = Sha256::new()
            .chain_update(b"kippu/denial/v1\0")
            .chain_update(organization.as_uuid().as_bytes())
            .chain_update(event.map_or([0; 16], |event| event.as_uuid().into_bytes()))
            .chain_update(subject.kind().as_bytes())
            .chain_update(subject.id().as_bytes())
            .finalize();
        let mut bytes = [0; 16];
        bytes.copy_from_slice(digest.get(..16).unwrap_or(&[0; 16]));
        Self::from_uuid(Uuid::new_v8(bytes))
    }
}

/// An entry of a deny list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Denial {
    /// Identity of the denial.
    pub id: DenialId,
    /// The organization whose list it is on.
    pub organization_id: OrganizationId,
    /// The event it applies to; `null` for every event of the organization, present and
    /// future.
    pub event_id: Option<EventId>,
    /// What is refused entry.
    pub subject: DenialSubject,
    /// Why, for staff; never shown to the person refused.
    pub note: String,
    /// When it was added.
    pub created_at: Timestamp,
    /// When it was last changed.
    pub updated_at: Timestamp,
    /// Incremented on every change, for optimistic concurrency.
    pub version: i64,
}

impl Denial {
    /// Checks the invariants of a denial's editable fields.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.note.chars().count() > MAX_NOTE_CHARS {
            return Err(ValidationError::new(
                "note",
                "must be at most 1000 characters",
            ));
        }
        Ok(())
    }
}

/// Why a gate must refuse a ticket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum DeniedBecause {
    /// The ticket was revoked: refunded, or its payment reversed.
    Revoked,
    /// The ticket, or the account holding it, is on a deny list.
    Denied,
}

impl DeniedBecause {
    /// The reason's name as stored and serialized.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Revoked => "revoked",
            Self::Denied => "denied",
        }
    }
}

impl std::str::FromStr for DeniedBecause {
    type Err = ValidationError;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        match name {
            "revoked" => Ok(Self::Revoked),
            "denied" => Ok(Self::Denied),
            _ => Err(ValidationError::new("reason", "must be revoked or denied")),
        }
    }
}

/// A ticket a gate must refuse, though its signature is valid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DeniedTicket {
    /// The ticket's id, as its `ticket_id` claim carries it.
    pub ticket_id: TicketId,
    /// Its ticket type.
    pub ticket_type_id: TicketTypeId,
    /// Why it is refused. A revoked ticket is reported as revoked even if it is also denied.
    pub reason: DeniedBecause,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subjects_serialize_with_their_kind() {
        let account = AccountId::generate();
        let json = serde_json::to_value(DenialSubject::Account(account)).unwrap();
        assert_eq!(json["kind"], "account");
        assert_eq!(json["id"], account.to_string());
        let back: DenialSubject = serde_json::from_value(json).unwrap();
        assert_eq!(back, DenialSubject::Account(account));
        assert_eq!(
            DenialSubject::from_parts(back.kind(), back.id()),
            Ok(DenialSubject::Account(account))
        );
        assert!(DenialSubject::from_parts("person", account.as_uuid()).is_err());
    }

    #[test]
    fn denial_ids_name_the_subject_in_its_place() {
        let organization = OrganizationId::generate();
        let event = EventId::generate();
        let subject = DenialSubject::Ticket(TicketId::generate());
        let id = DenialId::derive(organization, Some(event), subject);
        assert_eq!(id, DenialId::derive(organization, Some(event), subject));
        assert_ne!(id, DenialId::derive(organization, None, subject));
        assert_ne!(
            id,
            DenialId::derive(organization, Some(EventId::generate()), subject)
        );
        assert_ne!(
            id,
            DenialId::derive(
                organization,
                Some(event),
                DenialSubject::Account(AccountId::from_uuid(subject.id()))
            )
        );
    }

    #[test]
    fn notes_are_bounded() {
        let mut denial = Denial {
            id: DenialId::generate(),
            organization_id: OrganizationId::generate(),
            event_id: None,
            subject: DenialSubject::Account(AccountId::generate()),
            note: "x".repeat(MAX_NOTE_CHARS),
            created_at: Timestamp::UNIX_EPOCH,
            updated_at: Timestamp::UNIX_EPOCH,
            version: 1,
        };
        assert!(denial.validate().is_ok());
        denial.note.push('x');
        assert!(denial.validate().is_err());
    }
}
