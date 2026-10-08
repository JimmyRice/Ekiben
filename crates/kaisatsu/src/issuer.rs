use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use ed25519_dalek::{Signer, SigningKey};

use crate::key::{KeyId, TrustedKey};
use crate::ticket::Uuid;
use crate::wire::{
    self, ALGORITHM_ED25519, HEADER_LEN, MAGIC, MAX_CLAIM_LEN, MAX_ISSUER_LEN, MAX_TICKET_LEN,
    SIGNATURE_LEN, VERSION, encoded_claim_len, tag,
};

/// Everything a ticket asserts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claims {
    /// Who issues the ticket, e.g. `"kippu.example.org"`. UTF-8, 1–64 bytes.
    pub issuer: String,
    /// The event the ticket admits to.
    pub event_id: Uuid,
    /// The ticket's unique id.
    pub ticket_id: Uuid,
    /// The kind of ticket.
    pub ticket_type_id: Uuid,
    /// Start of validity (inclusive), in Unix seconds.
    pub valid_from: u64,
    /// End of validity (exclusive), in Unix seconds.
    pub valid_until: u64,
    /// When the ticket is issued, in Unix seconds.
    pub issued_at: u64,
    /// Application-defined claims, keyed by tag in `0x80..=0xFF`.
    pub extensions: BTreeMap<u8, Vec<u8>>,
}

/// Signs tickets with an Ed25519 secret key.
pub struct Issuer {
    signing_key: SigningKey,
    key_id: KeyId,
}

impl Issuer {
    /// Creates an issuer from a 32-byte Ed25519 secret key (seed).
    pub fn from_secret_key(secret_key: &[u8; 32]) -> Self {
        let signing_key = SigningKey::from_bytes(secret_key);
        let key_id = KeyId::of(signing_key.verifying_key().as_bytes());
        Self {
            signing_key,
            key_id,
        }
    }

    /// The id stamped into every ticket this issuer signs.
    pub const fn key_id(&self) -> KeyId {
        self.key_id
    }

    /// The public key verifiers need to trust.
    pub fn public_key(&self) -> [u8; 32] {
        self.signing_key.verifying_key().to_bytes()
    }

    /// The public key as a [`TrustedKey`], e.g. for a verifier in the same process.
    pub fn trusted_key(&self) -> TrustedKey {
        TrustedKey::from_verifying_key(self.signing_key.verifying_key())
    }

    /// Encodes and signs a ticket.
    ///
    /// Encoding is canonical and Ed25519 signatures are deterministic, so the same claims and
    /// key always produce the same bytes.
    pub fn issue(&self, claims: &Claims) -> Result<Vec<u8>, IssueError> {
        claims.validate()?;
        let len = claims.encoded_len();
        if len > MAX_TICKET_LEN {
            return Err(IssueError::TicketTooLong);
        }
        let mut ticket = Vec::with_capacity(len);
        ticket.extend_from_slice(&MAGIC);
        ticket.push(VERSION);
        ticket.push(ALGORITHM_ED25519);
        ticket.extend_from_slice(&self.key_id.to_bytes());

        push_claim(&mut ticket, tag::ISSUER, claims.issuer.as_bytes());
        push_claim(&mut ticket, tag::EVENT_ID, &claims.event_id.to_bytes());
        push_claim(&mut ticket, tag::TICKET_ID, &claims.ticket_id.to_bytes());
        push_claim(
            &mut ticket,
            tag::TICKET_TYPE_ID,
            &claims.ticket_type_id.to_bytes(),
        );
        push_claim(
            &mut ticket,
            tag::VALID_FROM,
            &claims.valid_from.to_be_bytes(),
        );
        push_claim(
            &mut ticket,
            tag::VALID_UNTIL,
            &claims.valid_until.to_be_bytes(),
        );
        push_claim(&mut ticket, tag::ISSUED_AT, &claims.issued_at.to_be_bytes());
        for (&tag, value) in &claims.extensions {
            push_claim(&mut ticket, tag, value);
        }

        Ok(self.sign_unchecked(ticket))
    }

    /// Appends a signature to arbitrary `header ‖ claims` bytes without validating them.
    ///
    /// Only for producing deliberately malformed test vectors; use [`Issuer::issue`] instead.
    #[doc(hidden)]
    pub fn sign_unchecked(&self, mut header_and_claims: Vec<u8>) -> Vec<u8> {
        let signature = self.signing_key.sign(&header_and_claims);
        header_and_claims.extend_from_slice(&signature.to_bytes());
        header_and_claims
    }
}

impl fmt::Debug for Issuer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Issuer")
            .field("key_id", &self.key_id)
            .finish_non_exhaustive()
    }
}

impl Claims {
    /// The length of the ticket these claims encode to, header and signature included.
    /// [`Issuer::issue`] refuses claims longer than [`MAX_TICKET_LEN`]; checking this first
    /// tells whether a ticket can be issued without signing one.
    pub fn encoded_len(&self) -> usize {
        let uuid = encoded_claim_len(16);
        let seconds = encoded_claim_len(8);
        self.extensions.values().fold(
            HEADER_LEN
                .saturating_add(encoded_claim_len(self.issuer.len()))
                .saturating_add(uuid.saturating_mul(3))
                .saturating_add(seconds.saturating_mul(3))
                .saturating_add(SIGNATURE_LEN),
            |len, value| len.saturating_add(encoded_claim_len(value.len())),
        )
    }

    fn validate(&self) -> Result<(), IssueError> {
        if self.issuer.is_empty() || self.issuer.len() > MAX_ISSUER_LEN {
            return Err(IssueError::InvalidIssuer);
        }
        if self.valid_until <= self.valid_from {
            return Err(IssueError::EmptyValidity);
        }
        for (&tag, value) in &self.extensions {
            if tag < tag::FIRST_EXTENSION {
                return Err(IssueError::ReservedTag(tag));
            }
            if value.len() > MAX_CLAIM_LEN {
                return Err(IssueError::ClaimTooLong(tag));
            }
        }
        Ok(())
    }
}

fn push_claim(out: &mut Vec<u8>, tag: u8, value: &[u8]) {
    out.push(tag);
    wire::push_claim_len(out, value.len());
    out.extend_from_slice(value);
}

/// Why a ticket could not be issued.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum IssueError {
    /// The issuer is empty or longer than 64 bytes.
    InvalidIssuer,
    /// `valid_until` is not after `valid_from`.
    EmptyValidity,
    /// An extension uses a tag below `0x80`, which the protocol reserves.
    ReservedTag(u8),
    /// A claim value is longer than [`MAX_CLAIM_LEN`].
    ClaimTooLong(u8),
    /// The encoded ticket exceeds [`MAX_TICKET_LEN`].
    TicketTooLong,
}

impl fmt::Display for IssueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidIssuer => f.write_str("issuer must be 1 to 64 bytes"),
            Self::EmptyValidity => f.write_str("valid_until must be after valid_from"),
            Self::ReservedTag(tag) => write!(f, "extension tag {tag:#04x} is reserved"),
            Self::ClaimTooLong(tag) => write!(f, "claim {tag:#04x} is too long"),
            Self::TicketTooLong => write!(f, "ticket exceeds {MAX_TICKET_LEN} bytes"),
        }
    }
}

impl core::error::Error for IssueError {}
