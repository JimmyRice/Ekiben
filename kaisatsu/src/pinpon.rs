use core::fmt;

use crate::key::KeyId;

/// Why a ticket was rejected.
///
/// Named after the *pinpōn* 🔔 a Japanese ticket gate sounds as it shuts its flaps on you:
///
/// ```
/// assert_eq!(kaisatsu::Pinpon::BadSignature.to_string(), "ピンポーン🔔 BadSignature");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Pinpon {
    /// The bytes are not a well-formed ticket.
    Malformed(Defect),
    /// The ticket uses a protocol version this library does not understand.
    UnsupportedVersion(u8),
    /// The ticket is signed with an algorithm this library does not understand.
    UnsupportedAlgorithm(u8),
    /// The ticket was signed by a key that is not in the verifier's key ring.
    UnknownKey(KeyId),
    /// The signature does not match: the ticket was forged or altered.
    BadSignature,
    /// The ticket's validity window has not started yet.
    NotYetValid,
    /// The ticket's validity window has ended.
    Expired,
}

impl Pinpon {
    /// The variant's name, e.g. `"BadSignature"`. Stable across releases.
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Malformed(_) => "Malformed",
            Self::UnsupportedVersion(_) => "UnsupportedVersion",
            Self::UnsupportedAlgorithm(_) => "UnsupportedAlgorithm",
            Self::UnknownKey(_) => "UnknownKey",
            Self::BadSignature => "BadSignature",
            Self::NotYetValid => "NotYetValid",
            Self::Expired => "Expired",
        }
    }
}

impl fmt::Display for Pinpon {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ピンポーン🔔 {}", self.name())?;
        match self {
            Self::Malformed(defect) => write!(f, " ({defect})"),
            Self::UnsupportedVersion(version) => write!(f, " (version {version})"),
            Self::UnsupportedAlgorithm(algorithm) => write!(f, " (algorithm {algorithm})"),
            Self::UnknownKey(key_id) => write!(f, " (key {key_id})"),
            Self::BadSignature | Self::NotYetValid | Self::Expired => Ok(()),
        }
    }
}

impl core::error::Error for Pinpon {}

impl From<Defect> for Pinpon {
    fn from(defect: Defect) -> Self {
        Self::Malformed(defect)
    }
}

/// What exactly is wrong with a malformed ticket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Defect {
    /// Shorter than a header plus a signature.
    TooShort,
    /// Longer than [`MAX_TICKET_LEN`](crate::wire::MAX_TICKET_LEN).
    TooLong,
    /// Does not start with `"KP"`.
    BadMagic,
    /// A claim runs past the end of the claims section.
    Truncated,
    /// A claim length is not in its shortest LEB128 form.
    NonCanonicalLength,
    /// Claim tags are not strictly ascending.
    ClaimsOutOfOrder,
    /// A claim in the reserved critical range that this version does not know.
    UnknownCriticalClaim(u8),
    /// A claim required by the protocol is absent.
    MissingClaim(u8),
    /// A claim has the wrong length or an invalid value.
    InvalidClaim(u8),
    /// The text is not valid Base45.
    InvalidBase45,
    /// The output buffer is too small for the decoded ticket.
    BufferTooSmall,
}

impl fmt::Display for Defect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooShort => f.write_str("too short"),
            Self::TooLong => f.write_str("too long"),
            Self::BadMagic => f.write_str("bad magic"),
            Self::Truncated => f.write_str("truncated claim"),
            Self::NonCanonicalLength => f.write_str("non-canonical claim length"),
            Self::ClaimsOutOfOrder => f.write_str("claims out of order"),
            Self::UnknownCriticalClaim(tag) => write!(f, "unknown critical claim {tag:#04x}"),
            Self::MissingClaim(tag) => write!(f, "missing claim {tag:#04x}"),
            Self::InvalidClaim(tag) => write!(f, "invalid claim {tag:#04x}"),
            Self::InvalidBase45 => f.write_str("invalid Base45"),
            Self::BufferTooSmall => f.write_str("buffer too small"),
        }
    }
}

impl core::error::Error for Defect {}
