//! Parsing keys from configuration, and the ticket signing key ring.

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};
use ed25519_dalek::pkcs8::{DecodePrivateKey, DecodePublicKey};
use ed25519_dalek::{SigningKey, VerifyingKey};
use kaisatsu::{Issuer, KeyId};
use secrecy::{ExposeSecret, SecretString};
use serde::Serialize;

/// A key in the configuration could not be used.
#[derive(Debug, thiserror::Error)]
#[error("{name}: {reason}")]
pub struct KeyError {
    /// Which configuration value.
    pub name: String,
    /// What is wrong with it.
    pub reason: &'static str,
}

fn decode_base64(text: &str) -> Option<Vec<u8>> {
    let text = text.trim();
    [STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD]
        .iter()
        .find_map(|engine| engine.decode(text).ok())
}

/// Parses an Ed25519 secret key: a base64 32-byte seed, or PKCS#8 PEM (as written by
/// `openssl genpkey -algorithm ed25519`).
pub fn parse_signing_key(name: &str, value: &SecretString) -> Result<SigningKey, KeyError> {
    let text = value.expose_secret().trim();
    let error = |reason| KeyError {
        name: name.to_owned(),
        reason,
    };
    if text.starts_with("-----BEGIN") {
        return SigningKey::from_pkcs8_pem(text)
            .map_err(|_| error("not a valid PKCS#8 Ed25519 private key"));
    }
    let bytes = decode_base64(text).ok_or_else(|| error("not valid base64"))?;
    let seed: [u8; 32] = bytes
        .try_into()
        .map_err(|_| error("must be a 32-byte Ed25519 seed"))?;
    Ok(SigningKey::from_bytes(&seed))
}

/// Parses an Ed25519 public key: 32 bytes in base64, or SPKI PEM.
pub fn parse_verifying_key(name: &str, text: &str) -> Result<VerifyingKey, KeyError> {
    let text = text.trim();
    let error = |reason| KeyError {
        name: name.to_owned(),
        reason,
    };
    let key = if text.starts_with("-----BEGIN") {
        VerifyingKey::from_public_key_pem(text)
            .map_err(|_| error("not a valid SPKI Ed25519 public key"))?
    } else {
        let bytes = decode_base64(text).ok_or_else(|| error("not valid base64"))?;
        let bytes: [u8; 32] = bytes.try_into().map_err(|_| error("must be 32 bytes"))?;
        VerifyingKey::from_bytes(&bytes).map_err(|_| error("not a valid Ed25519 public key"))?
    };
    if key.is_weak() {
        return Err(error("is a weak key"));
    }
    Ok(key)
}

/// Encodes key bytes the way configuration and API responses expect them.
pub fn encode_key(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

/// The ticket signing key and the retired keys still published for verifiers.
pub struct TicketKeys {
    issuer: Issuer,
    retired: Vec<VerifyingKey>,
}

/// One entry of `/.well-known/kippu/ticket-keys`.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct PublishedKey {
    /// The key id stamped into ticket headers, hex.
    pub key_id: String,
    /// Always `"Ed25519"`.
    pub algorithm: &'static str,
    /// The 32-byte public key, base64.
    pub public_key: String,
    /// `active` keys sign new tickets; `retired` keys only verify older ones.
    pub status: &'static str,
}

impl TicketKeys {
    /// Builds the key ring from configuration.
    pub fn new(signing_key: &SigningKey, retired: Vec<VerifyingKey>) -> Self {
        Self {
            issuer: Issuer::from_secret_key(signing_key.as_bytes()),
            retired,
        }
    }

    /// Signs tickets.
    pub fn issuer(&self) -> &Issuer {
        &self.issuer
    }

    /// Every key verifiers should trust, the active one first.
    pub fn published(&self) -> Vec<PublishedKey> {
        let active = PublishedKey {
            key_id: self.issuer.key_id().to_string(),
            algorithm: "Ed25519",
            public_key: encode_key(&self.issuer.public_key()),
            status: "active",
        };
        let retired = self.retired.iter().map(|key| PublishedKey {
            key_id: KeyId::of(key.as_bytes()).to_string(),
            algorithm: "Ed25519",
            public_key: encode_key(key.as_bytes()),
            status: "retired",
        });
        std::iter::once(active).chain(retired).collect()
    }
}
