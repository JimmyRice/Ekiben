#[cfg(feature = "base45")]
use crate::base45;
use crate::key::{KeyId, KeyRing};
use crate::pinpon::{Defect, Pinpon};
use crate::ticket::VerifiedTicket;
use crate::wire::{ALGORITHM_ED25519, MAGIC, MAX_TICKET_LEN, Reader, SIGNATURE_LEN, VERSION};

/// Checks that tickets were signed by a trusted key.
///
/// ```no_run
/// use kaisatsu::{TrustedKey, Verifier};
///
/// fn admit(public_key: &[u8; 32], scanned: &[u8], now: u64) -> bool {
///     let Ok(key) = TrustedKey::from_bytes(public_key) else { return false };
///     let verifier = Verifier::new(key);
///     match verifier.verify(scanned) {
///         Ok(ticket) => ticket.check_time(now).is_ok(),
///         Err(pinpon) => {
///             eprintln!("{pinpon}"); // ピンポーン🔔 BadSignature
///             false
///         }
///     }
/// }
/// ```
#[derive(Debug, Clone)]
pub struct Verifier<K> {
    keys: K,
}

impl<K: KeyRing> Verifier<K> {
    /// Creates a verifier that trusts the keys in `keys`.
    pub const fn new(keys: K) -> Self {
        Self { keys }
    }

    /// The keys this verifier trusts.
    pub const fn keys(&self) -> &K {
        &self.keys
    }

    /// Verifies a binary ticket.
    ///
    /// The header is checked first, then the signature, and only then are the claims parsed —
    /// so nothing beyond the fixed-size header is interpreted until it is known to be authentic.
    pub fn verify<'t>(&self, ticket: &'t [u8]) -> Result<VerifiedTicket<'t>, Pinpon> {
        let envelope = Envelope::open(ticket)?;
        let key = self
            .keys
            .find(envelope.key_id)
            .ok_or(Pinpon::UnknownKey(envelope.key_id))?;
        key.verify(envelope.signed, envelope.signature)?;
        Ok(VerifiedTicket::parse(envelope.key_id, envelope.claims)?)
    }

    /// Verifies a ticket carried as Base45 text, decoding into `buffer`.
    ///
    /// A buffer of [`MAX_TICKET_LEN`] bytes always suffices.
    #[cfg(feature = "base45")]
    pub fn verify_base45<'b>(
        &self,
        text: &str,
        buffer: &'b mut [u8],
    ) -> Result<VerifiedTicket<'b>, Pinpon> {
        let ticket = base45::decode_into(text.as_bytes(), buffer)?;
        self.verify(ticket)
    }
}

/// A ticket split into its parts, before anything but the header has been interpreted.
struct Envelope<'t> {
    key_id: KeyId,
    /// Header and claims: the bytes covered by the signature.
    signed: &'t [u8],
    claims: &'t [u8],
    signature: &'t [u8; SIGNATURE_LEN],
}

impl<'t> Envelope<'t> {
    fn open(ticket: &'t [u8]) -> Result<Self, Pinpon> {
        if ticket.len() > MAX_TICKET_LEN {
            return Err(Defect::TooLong.into());
        }
        let (signed, signature) = ticket
            .split_last_chunk::<SIGNATURE_LEN>()
            .ok_or(Defect::TooShort)?;

        let mut header = Reader::new(signed);
        if header.array::<2>().ok_or(Defect::TooShort)? != MAGIC {
            return Err(Defect::BadMagic.into());
        }
        let version = header.u8().ok_or(Defect::TooShort)?;
        if version != VERSION {
            return Err(Pinpon::UnsupportedVersion(version));
        }
        let algorithm = header.u8().ok_or(Defect::TooShort)?;
        if algorithm != ALGORITHM_ED25519 {
            return Err(Pinpon::UnsupportedAlgorithm(algorithm));
        }
        let key_id = KeyId::from_bytes(header.array::<8>().ok_or(Defect::TooShort)?);

        Ok(Self {
            key_id,
            signed,
            claims: header.remaining(),
            signature,
        })
    }
}
