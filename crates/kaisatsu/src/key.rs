use core::fmt;

use ed25519_dalek::{Signature, VerifyingKey};
use sha2::{Digest, Sha512};

use crate::pinpon::Pinpon;

/// Short identifier of a signing key, carried in every ticket header.
///
/// Derived as the first eight bytes of `SHA-512(public_key)`, so issuers never have to
/// coordinate key ids and verifiers can compute them from the public key alone.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct KeyId([u8; 8]);

impl KeyId {
    /// Derives the key id of an Ed25519 public key.
    pub fn of(public_key: &[u8; 32]) -> Self {
        let digest = Sha512::digest(public_key);
        let mut id = [0; 8];
        for (slot, byte) in id.iter_mut().zip(digest.iter()) {
            *slot = *byte;
        }
        Self(id)
    }

    /// Wraps raw key id bytes, e.g. as read from a ticket header.
    pub const fn from_bytes(bytes: [u8; 8]) -> Self {
        Self(bytes)
    }

    /// The raw key id bytes.
    pub const fn to_bytes(self) -> [u8; 8] {
        self.0
    }
}

impl fmt::Display for KeyId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.iter().try_for_each(|byte| write!(f, "{byte:02x}"))
    }
}

impl fmt::Debug for KeyId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "KeyId({self})")
    }
}

/// An Ed25519 public key the verifier trusts to sign tickets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrustedKey {
    id: KeyId,
    key: VerifyingKey,
}

impl TrustedKey {
    /// Parses a 32-byte Ed25519 public key.
    ///
    /// Rejects byte strings that are not a valid curve point, as well as weak (small-order)
    /// keys under which signatures would be meaningless.
    pub fn from_bytes(public_key: &[u8; 32]) -> Result<Self, InvalidKey> {
        let key = VerifyingKey::from_bytes(public_key).map_err(|_| InvalidKey)?;
        if key.is_weak() {
            return Err(InvalidKey);
        }
        Ok(Self {
            id: KeyId::of(public_key),
            key,
        })
    }

    /// Wraps a key derived from a secret key, which is always a valid, non-weak point.
    #[cfg(feature = "issuer")]
    pub(crate) fn from_verifying_key(key: VerifyingKey) -> Self {
        Self {
            id: KeyId::of(key.as_bytes()),
            key,
        }
    }

    /// The key id tickets signed by this key carry.
    pub const fn id(&self) -> KeyId {
        self.id
    }

    /// The 32-byte public key.
    pub fn to_bytes(&self) -> [u8; 32] {
        self.key.to_bytes()
    }

    pub(crate) fn verify(&self, message: &[u8], signature: &[u8; 64]) -> Result<(), Pinpon> {
        self.key
            .verify_strict(message, &Signature::from_bytes(signature))
            .map_err(|_| Pinpon::BadSignature)
    }
}

/// The public key is not a usable Ed25519 key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidKey;

impl fmt::Display for InvalidKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("not a valid Ed25519 public key")
    }
}

impl core::error::Error for InvalidKey {}

/// A set of trusted keys, looked up by [`KeyId`].
///
/// Implemented for a single key, arrays, slices and (with `alloc`) vectors. Implement it
/// yourself to look keys up somewhere else, such as a key store in microcontroller flash.
pub trait KeyRing {
    /// Returns the trusted key with the given id, if there is one.
    fn find(&self, id: KeyId) -> Option<TrustedKey>;
}

impl KeyRing for TrustedKey {
    fn find(&self, id: KeyId) -> Option<TrustedKey> {
        (self.id == id).then_some(*self)
    }
}

impl KeyRing for [TrustedKey] {
    fn find(&self, id: KeyId) -> Option<TrustedKey> {
        self.iter().find(|key| key.id == id).copied()
    }
}

impl<const N: usize> KeyRing for [TrustedKey; N] {
    fn find(&self, id: KeyId) -> Option<TrustedKey> {
        self.as_slice().find(id)
    }
}

#[cfg(feature = "alloc")]
impl KeyRing for alloc::vec::Vec<TrustedKey> {
    fn find(&self, id: KeyId) -> Option<TrustedKey> {
        self.as_slice().find(id)
    }
}

impl<K: KeyRing + ?Sized> KeyRing for &K {
    fn find(&self, id: KeyId) -> Option<TrustedKey> {
        (**self).find(id)
    }
}
