use kaisatsu::Pinpon;

/// Why a ticket (or a key) was rejected.
///
/// The variants mirror `kaisatsu::Pinpon`; the message is the same text the C library gives,
/// such as `ピンポーン🔔 BadSignature`.
#[derive(Debug, uniffi::Error)]
#[uniffi(flat_error)]
pub enum KaisatsuError {
    /// The bytes are not a well-formed ticket.
    Malformed(String),
    /// Unknown protocol version.
    UnsupportedVersion(String),
    /// Unknown signature algorithm.
    UnsupportedAlgorithm(String),
    /// Signed by a key that is not trusted.
    UnknownKey(String),
    /// The ticket was forged or altered.
    BadSignature(String),
    /// The validity window has not started.
    NotYetValid(String),
    /// The validity window has ended.
    Expired(String),
    /// A public key is not a usable Ed25519 key.
    InvalidKey(String),
}

impl core::fmt::Display for KaisatsuError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let (Self::Malformed(message)
        | Self::UnsupportedVersion(message)
        | Self::UnsupportedAlgorithm(message)
        | Self::UnknownKey(message)
        | Self::BadSignature(message)
        | Self::NotYetValid(message)
        | Self::Expired(message)
        | Self::InvalidKey(message)) = self;
        f.write_str(message)
    }
}

impl core::error::Error for KaisatsuError {}

impl From<Pinpon> for KaisatsuError {
    fn from(pinpon: Pinpon) -> Self {
        let message = pinpon.to_string();
        match pinpon {
            Pinpon::UnsupportedVersion(_) => Self::UnsupportedVersion(message),
            Pinpon::UnsupportedAlgorithm(_) => Self::UnsupportedAlgorithm(message),
            Pinpon::UnknownKey(_) => Self::UnknownKey(message),
            Pinpon::BadSignature => Self::BadSignature(message),
            Pinpon::NotYetValid => Self::NotYetValid(message),
            Pinpon::Expired => Self::Expired(message),
            // `Malformed` and any variant added in a future version of Kaisatsu.
            _ => Self::Malformed(message),
        }
    }
}
