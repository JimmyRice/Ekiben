use core::fmt;

use crate::key::KeyId;
use crate::pinpon::{Defect, Pinpon};
use crate::wire::{self, MAX_ISSUER_LEN, Reader, tag};

/// A 16-byte UUID, displayed in the usual hyphenated form.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Uuid([u8; 16]);

impl Uuid {
    /// Wraps raw UUID bytes.
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    /// The raw UUID bytes, ready for `uuid::Uuid::from_bytes` or a database column.
    pub const fn to_bytes(self) -> [u8; 16] {
        self.0
    }
}

impl fmt::Display for Uuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, byte) in self.0.iter().enumerate() {
            if matches!(index, 4 | 6 | 8 | 10) {
                f.write_str("-")?;
            }
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for Uuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Uuid({self})")
    }
}

/// A ticket whose signature has been checked against a trusted key.
///
/// It borrows from the bytes it was verified from; nothing is copied or allocated.
/// Verification says nothing about *policy* — whether the ticket was already used, whether
/// this gate admits its ticket type — which is for the embedding application to decide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerifiedTicket<'t> {
    key_id: KeyId,
    issuer: &'t str,
    event_id: Uuid,
    ticket_id: Uuid,
    ticket_type_id: Uuid,
    valid_from: u64,
    valid_until: u64,
    issued_at: u64,
    extensions: Extensions<'t>,
}

impl<'t> VerifiedTicket<'t> {
    /// The protocol version of the ticket.
    pub const fn version(&self) -> u8 {
        wire::VERSION
    }

    /// The id of the key that signed the ticket.
    pub const fn key_id(&self) -> KeyId {
        self.key_id
    }

    /// Who issued the ticket, e.g. `"kippu.example.org"`.
    pub const fn issuer(&self) -> &'t str {
        self.issuer
    }

    /// The event the ticket admits to.
    pub const fn event_id(&self) -> Uuid {
        self.event_id
    }

    /// The ticket's unique id. Use it to detect re-use.
    pub const fn ticket_id(&self) -> Uuid {
        self.ticket_id
    }

    /// The kind of ticket, e.g. a one-day pass.
    pub const fn ticket_type_id(&self) -> Uuid {
        self.ticket_type_id
    }

    /// Start of validity (inclusive), in Unix seconds.
    pub const fn valid_from(&self) -> u64 {
        self.valid_from
    }

    /// End of validity (exclusive), in Unix seconds.
    pub const fn valid_until(&self) -> u64 {
        self.valid_until
    }

    /// When the ticket was issued, in Unix seconds.
    pub const fn issued_at(&self) -> u64 {
        self.issued_at
    }

    /// Application-defined claims (tags `0x80..=0xFF`).
    pub const fn extensions(&self) -> Extensions<'t> {
        self.extensions
    }

    /// Checks that `now` (Unix seconds) lies within `valid_from..valid_until`.
    ///
    /// Kept separate from verification because not every device has a trustworthy clock.
    pub const fn check_time(&self, now: u64) -> Result<(), Pinpon> {
        if now < self.valid_from {
            Err(Pinpon::NotYetValid)
        } else if now >= self.valid_until {
            Err(Pinpon::Expired)
        } else {
            Ok(())
        }
    }

    /// Parses the claims section of an authenticated ticket.
    pub(crate) fn parse(key_id: KeyId, claims: &'t [u8]) -> Result<Self, Defect> {
        let mut reader = Reader::new(claims);
        let mut builder = Builder::default();
        let mut previous_tag: Option<u8> = None;

        loop {
            let before = reader.clone();
            if reader.is_empty() {
                break;
            }
            let (tag, value) = reader.claim()?;
            if previous_tag.is_some_and(|previous| tag <= previous) {
                return Err(Defect::ClaimsOutOfOrder);
            }
            previous_tag = Some(tag);

            if tag >= tag::FIRST_EXTENSION {
                // Tags ascend, so every remaining claim is an extension.
                let extensions = Extensions::from_bytes(before.remaining())?;
                return builder.finish(key_id, extensions);
            }
            builder.accept(tag, value)?;
        }
        builder.finish(key_id, Extensions::EMPTY)
    }
}

/// Collects the protocol claims while the claims section is being read.
#[derive(Default)]
struct Builder<'t> {
    issuer: Option<&'t str>,
    event_id: Option<Uuid>,
    ticket_id: Option<Uuid>,
    ticket_type_id: Option<Uuid>,
    valid_from: Option<u64>,
    valid_until: Option<u64>,
    issued_at: Option<u64>,
}

impl<'t> Builder<'t> {
    fn accept(&mut self, tag: u8, value: &'t [u8]) -> Result<(), Defect> {
        match tag {
            tag::ISSUER => self.issuer = Some(issuer(value)?),
            tag::EVENT_ID => self.event_id = Some(uuid(tag, value)?),
            tag::TICKET_ID => self.ticket_id = Some(uuid(tag, value)?),
            tag::TICKET_TYPE_ID => self.ticket_type_id = Some(uuid(tag, value)?),
            tag::VALID_FROM => self.valid_from = Some(timestamp(tag, value)?),
            tag::VALID_UNTIL => self.valid_until = Some(timestamp(tag, value)?),
            tag::ISSUED_AT => self.issued_at = Some(timestamp(tag, value)?),
            tag::FIRST_RESERVED_CRITICAL..tag::FIRST_RESERVED_OPTIONAL => {
                return Err(Defect::UnknownCriticalClaim(tag));
            }
            // Reserved non-critical claims from a future minor revision: safe to ignore.
            _ => {}
        }
        Ok(())
    }

    fn finish(
        self,
        key_id: KeyId,
        extensions: Extensions<'t>,
    ) -> Result<VerifiedTicket<'t>, Defect> {
        let valid_from = self
            .valid_from
            .ok_or(Defect::MissingClaim(tag::VALID_FROM))?;
        let valid_until = self
            .valid_until
            .ok_or(Defect::MissingClaim(tag::VALID_UNTIL))?;
        if valid_until <= valid_from {
            return Err(Defect::InvalidClaim(tag::VALID_UNTIL));
        }
        Ok(VerifiedTicket {
            key_id,
            issuer: self.issuer.ok_or(Defect::MissingClaim(tag::ISSUER))?,
            event_id: self.event_id.ok_or(Defect::MissingClaim(tag::EVENT_ID))?,
            ticket_id: self.ticket_id.ok_or(Defect::MissingClaim(tag::TICKET_ID))?,
            ticket_type_id: self
                .ticket_type_id
                .ok_or(Defect::MissingClaim(tag::TICKET_TYPE_ID))?,
            valid_from,
            valid_until,
            issued_at: self.issued_at.ok_or(Defect::MissingClaim(tag::ISSUED_AT))?,
            extensions,
        })
    }
}

fn issuer(value: &[u8]) -> Result<&str, Defect> {
    let invalid = Defect::InvalidClaim(tag::ISSUER);
    if value.is_empty() || value.len() > MAX_ISSUER_LEN {
        return Err(invalid);
    }
    core::str::from_utf8(value).map_err(|_| invalid)
}

fn uuid(tag: u8, value: &[u8]) -> Result<Uuid, Defect> {
    let bytes = <[u8; 16]>::try_from(value).map_err(|_| Defect::InvalidClaim(tag))?;
    Ok(Uuid(bytes))
}

fn timestamp(tag: u8, value: &[u8]) -> Result<u64, Defect> {
    let bytes = <[u8; 8]>::try_from(value).map_err(|_| Defect::InvalidClaim(tag))?;
    Ok(u64::from_be_bytes(bytes))
}

/// The application-defined claims of a ticket, in ascending tag order.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Extensions<'t> {
    raw: &'t [u8],
}

impl<'t> Extensions<'t> {
    const EMPTY: Self = Self { raw: &[] };

    /// Validates a run of extension claims in their wire encoding.
    ///
    /// Bindings use this to rebuild [`Extensions`] from [`Extensions::as_bytes`].
    pub fn from_bytes(raw: &'t [u8]) -> Result<Self, Defect> {
        let mut reader = Reader::new(raw);
        let mut previous_tag: Option<u8> = None;
        while !reader.is_empty() {
            let (tag, _) = reader.claim()?;
            if previous_tag.is_some_and(|previous| tag <= previous) {
                return Err(Defect::ClaimsOutOfOrder);
            }
            previous_tag = Some(tag);
        }
        Ok(Self { raw })
    }

    /// The value of the extension with the given tag.
    pub fn get(&self, tag: u8) -> Option<&'t [u8]> {
        self.iter()
            .find(|extension| extension.tag == tag)
            .map(|extension| extension.value)
    }

    /// Iterates over the extensions in ascending tag order.
    pub fn iter(&self) -> ExtensionIter<'t> {
        ExtensionIter {
            reader: Reader::new(self.raw),
        }
    }

    /// Whether the ticket carries no extensions.
    pub const fn is_empty(&self) -> bool {
        self.raw.is_empty()
    }

    /// The extensions in their wire encoding: `tag ‖ length ‖ value` records.
    pub const fn as_bytes(&self) -> &'t [u8] {
        self.raw
    }
}

impl fmt::Debug for Extensions<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

impl<'t> IntoIterator for Extensions<'t> {
    type Item = Extension<'t>;
    type IntoIter = ExtensionIter<'t>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'t> IntoIterator for &Extensions<'t> {
    type Item = Extension<'t>;
    type IntoIter = ExtensionIter<'t>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// One application-defined claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Extension<'t> {
    /// The claim tag, in `0x80..=0xFF`.
    pub tag: u8,
    /// The claim value, uninterpreted.
    pub value: &'t [u8],
}

/// Iterator over [`Extensions`].
#[derive(Debug, Clone)]
pub struct ExtensionIter<'t> {
    reader: Reader<'t>,
}

impl<'t> Iterator for ExtensionIter<'t> {
    type Item = Extension<'t>;

    fn next(&mut self) -> Option<Self::Item> {
        // The run was validated when the ticket was parsed, so reads cannot fail here.
        let (tag, value) = self.reader.claim().ok()?;
        Some(Extension { tag, value })
    }
}
