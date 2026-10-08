//! Constants and primitives of the KP1 wire format.
//!
//! ```text
//! Ticket    = Header ‖ Claims ‖ Signature
//! Header    = "KP" ‖ version(u8) ‖ algorithm(u8) ‖ key_id(8 bytes)        — 12 bytes
//! Claims    = { tag(u8) ‖ length(LEB128, 1–2 bytes) ‖ value }*            — tags strictly ascending
//! Signature = Ed25519(secret_key, Header ‖ Claims)                         — 64 bytes
//! ```
//!
//! The normative specification lives in `spec/ticket-protocol.md`.

use crate::pinpon::Defect;

/// The two bytes every ticket starts with.
pub const MAGIC: [u8; 2] = *b"KP";
/// The protocol version this crate reads and writes.
pub const VERSION: u8 = 1;
/// Algorithm identifier for Ed25519 signatures, the only algorithm in version 1.
pub const ALGORITHM_ED25519: u8 = 1;

/// Length of the fixed header: magic, version, algorithm and key id.
pub const HEADER_LEN: usize = 12;
/// Length of the trailing Ed25519 signature.
pub const SIGNATURE_LEN: usize = 64;
/// Upper bound on an encoded ticket. Comfortably fits in a QR code.
pub const MAX_TICKET_LEN: usize = 2048;
/// Upper bound on a single claim value: the largest two-byte LEB128 length.
pub const MAX_CLAIM_LEN: usize = 0x3FFF;
/// Upper bound on the `issuer` claim, in bytes.
pub const MAX_ISSUER_LEN: usize = 64;

/// Claim tags defined by version 1 of the protocol.
pub mod tag {
    /// Who issued the ticket. UTF-8, 1–64 bytes.
    pub const ISSUER: u8 = 0x01;
    /// The event the ticket admits to. 16-byte UUID.
    pub const EVENT_ID: u8 = 0x02;
    /// The ticket's own identity. 16-byte UUID.
    pub const TICKET_ID: u8 = 0x03;
    /// The kind of ticket (e.g. one-day pass). 16-byte UUID.
    pub const TICKET_TYPE_ID: u8 = 0x04;
    /// Start of validity, inclusive. Unix seconds, `u64` big-endian.
    pub const VALID_FROM: u8 = 0x05;
    /// End of validity, exclusive. Unix seconds, `u64` big-endian.
    pub const VALID_UNTIL: u8 = 0x06;
    /// When the ticket was issued. Unix seconds, `u64` big-endian.
    pub const ISSUED_AT: u8 = 0x07;

    /// First tag of the reserved *critical* range: unknown tags here are rejected.
    pub const FIRST_RESERVED_CRITICAL: u8 = 0x08;
    /// First tag of the reserved *non-critical* range: unknown tags here are skipped.
    pub const FIRST_RESERVED_OPTIONAL: u8 = 0x40;
    /// First application-defined extension tag. Extensions are passed through verbatim.
    pub const FIRST_EXTENSION: u8 = 0x80;
}

/// A cursor over a byte slice whose reads can fail but never panic.
#[derive(Debug, Clone)]
pub(crate) struct Reader<'a> {
    rest: &'a [u8],
}

impl<'a> Reader<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self { rest: bytes }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.rest.is_empty()
    }

    pub(crate) fn remaining(&self) -> &'a [u8] {
        self.rest
    }

    pub(crate) fn u8(&mut self) -> Option<u8> {
        let (&byte, rest) = self.rest.split_first()?;
        self.rest = rest;
        Some(byte)
    }

    pub(crate) fn array<const N: usize>(&mut self) -> Option<[u8; N]> {
        let (head, rest) = self.rest.split_first_chunk::<N>()?;
        self.rest = rest;
        Some(*head)
    }

    pub(crate) fn bytes(&mut self, len: usize) -> Option<&'a [u8]> {
        let (head, rest) = self.rest.split_at_checked(len)?;
        self.rest = rest;
        Some(head)
    }

    /// Reads one `tag ‖ length ‖ value` record.
    pub(crate) fn claim(&mut self) -> Result<(u8, &'a [u8]), Defect> {
        let tag = self.u8().ok_or(Defect::Truncated)?;
        let len = self.claim_len()?;
        let value = self.bytes(len).ok_or(Defect::Truncated)?;
        Ok((tag, value))
    }

    /// Reads a canonical LEB128 length of one or two bytes.
    fn claim_len(&mut self) -> Result<usize, Defect> {
        let low = self.u8().ok_or(Defect::Truncated)?;
        if low < 0x80 {
            return Ok(usize::from(low));
        }
        let high = self.u8().ok_or(Defect::Truncated)?;
        // `high == 0` would encode a value that fits in one byte; `high >= 0x80` would need a
        // third byte. Both are rejected so that every length has exactly one encoding.
        if high == 0 || high >= 0x80 {
            return Err(Defect::NonCanonicalLength);
        }
        Ok(join_len(low, high))
    }
}

/// Joins the two 7-bit halves of a two-byte LEB128 length.
fn join_len(low: u8, high: u8) -> usize {
    usize::from(low & 0x7F) | (usize::from(high) << 7)
}

/// How many bytes a claim of `len` value bytes takes: tag, length and value.
#[cfg(feature = "issuer")]
pub(crate) const fn encoded_claim_len(len: usize) -> usize {
    // Tag byte plus a one- or two-byte length.
    let overhead = if len < 0x80 { 2 } else { 3 };
    len.saturating_add(overhead)
}

/// Appends a canonical LEB128 encoding of `len` (at most [`MAX_CLAIM_LEN`]) to `out`.
#[cfg(feature = "issuer")]
#[expect(
    clippy::cast_possible_truncation,
    reason = "both halves are masked to 7 bits"
)]
pub(crate) fn push_claim_len(out: &mut alloc::vec::Vec<u8>, len: usize) {
    debug_assert!(len <= MAX_CLAIM_LEN);
    let low = (len & 0x7F) as u8;
    let high = ((len >> 7) & 0x7F) as u8;
    if high == 0 {
        out.push(low);
    } else {
        out.push(low | 0x80);
        out.push(high);
    }
}
