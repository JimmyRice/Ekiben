//! Plain data types shared with C. Everything here is `#[repr(C)]` and safe to copy.

use kaisatsu::{Defect, KeyId, KeyRing, Pinpon, TrustedKey, VerifiedTicket};

/// A trusted Ed25519 public key.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct KaisatsuKey {
    /// The 32-byte public key, as published at `/.well-known/kippu/ticket-keys`.
    pub public_key: [u8; 32],
}

/// The claims of a verified ticket.
///
/// `issuer` and `extensions` point into the ticket buffer passed to `kaisatsu_verify` and are
/// valid only as long as that buffer is.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct KaisatsuTicket {
    /// Protocol version.
    pub version: u8,
    /// Id of the key that signed the ticket.
    pub key_id: [u8; 8],
    /// Event the ticket admits to (UUID bytes).
    pub event_id: [u8; 16],
    /// Unique ticket id (UUID bytes). Use it to detect re-use.
    pub ticket_id: [u8; 16],
    /// Ticket type (UUID bytes).
    pub ticket_type_id: [u8; 16],
    /// Start of validity, inclusive, in Unix seconds.
    pub valid_from: u64,
    /// End of validity, exclusive, in Unix seconds.
    pub valid_until: u64,
    /// Issue time in Unix seconds.
    pub issued_at: u64,
    /// UTF-8 issuer, *not* NUL-terminated.
    pub issuer: *const u8,
    /// Length of `issuer` in bytes.
    pub issuer_len: usize,
    /// Extensions in wire encoding; read them with `kaisatsu_ticket_extension`.
    pub extensions: *const u8,
    /// Length of `extensions` in bytes.
    pub extensions_len: usize,
}

impl From<&VerifiedTicket<'_>> for KaisatsuTicket {
    fn from(ticket: &VerifiedTicket<'_>) -> Self {
        Self {
            version: ticket.version(),
            key_id: ticket.key_id().to_bytes(),
            event_id: ticket.event_id().to_bytes(),
            ticket_id: ticket.ticket_id().to_bytes(),
            ticket_type_id: ticket.ticket_type_id().to_bytes(),
            valid_from: ticket.valid_from(),
            valid_until: ticket.valid_until(),
            issued_at: ticket.issued_at(),
            issuer: ticket.issuer().as_ptr(),
            issuer_len: ticket.issuer().len(),
            extensions: ticket.extensions().as_bytes().as_ptr(),
            extensions_len: ticket.extensions().as_bytes().len(),
        }
    }
}

/// Result of a Kaisatsu call. `KAISATSU_STATUS_OK` is zero; everything else is a rejection.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KaisatsuStatus {
    /// Success.
    Ok = 0,
    /// The bytes are not a well-formed ticket (or not valid Base45).
    Malformed = 1,
    /// Unknown protocol version.
    UnsupportedVersion = 2,
    /// Unknown signature algorithm.
    UnsupportedAlgorithm = 3,
    /// Signed by a key that is not trusted.
    UnknownKey = 4,
    /// The ticket was forged or altered.
    BadSignature = 5,
    /// The validity window has not started.
    NotYetValid = 6,
    /// The validity window has ended.
    Expired = 7,
    /// A public key is not a usable Ed25519 key.
    InvalidKey = 8,
    /// A required pointer argument was null.
    NullPointer = 9,
    /// An output buffer is too small.
    BufferTooSmall = 10,
}

impl From<Pinpon> for KaisatsuStatus {
    fn from(pinpon: Pinpon) -> Self {
        match pinpon {
            Pinpon::Malformed(Defect::BufferTooSmall) => Self::BufferTooSmall,
            Pinpon::UnsupportedVersion(_) => Self::UnsupportedVersion,
            Pinpon::UnsupportedAlgorithm(_) => Self::UnsupportedAlgorithm,
            Pinpon::UnknownKey(_) => Self::UnknownKey,
            Pinpon::BadSignature => Self::BadSignature,
            Pinpon::NotYetValid => Self::NotYetValid,
            Pinpon::Expired => Self::Expired,
            // `Malformed` and any variant added in a future version of Kaisatsu.
            _ => Self::Malformed,
        }
    }
}

impl From<Defect> for KaisatsuStatus {
    fn from(defect: Defect) -> Self {
        Pinpon::Malformed(defect).into()
    }
}

impl<T> From<Result<T, Pinpon>> for KaisatsuStatus {
    fn from(result: Result<T, Pinpon>) -> Self {
        result.map_or_else(Self::from, |_| Self::Ok)
    }
}

/// A key ring backed by raw public keys from C. Keys are parsed lazily, only when a ticket's
/// key id matches, so callers need no setup step and nothing has to be freed.
pub(crate) struct RawKeys<'a>(pub(crate) &'a [KaisatsuKey]);

impl KeyRing for RawKeys<'_> {
    fn find(&self, id: KeyId) -> Option<TrustedKey> {
        self.0
            .iter()
            .filter(|key| KeyId::of(&key.public_key) == id)
            .find_map(|key| TrustedKey::from_bytes(&key.public_key).ok())
    }
}
