//! Signed webhook deliveries.
//!
//! Every delivery carries an Ed25519 signature by the deployment's webhook key:
//!
//! ```text
//! Kippu-Signature: key=<key id>, ts=<unix seconds>, sig=<base64url>
//!
//! signed message = "kippu-webhook-v1" \n URL \n ts \n hex(SHA-256(body))
//! ```
//!
//! `URL` is the webhook's URL exactly as registered, so a delivery cannot be replayed to
//! another endpoint. Receivers check the signature with the key published at
//! `/.well-known/kippu/webhook-keys`, reject timestamps far from their clock, and drop
//! duplicates by `Kippu-Delivery` (deliveries are at least once). See
//! `spec/webhook-protocol.md`.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use kaisatsu::KeyId;
use sha2::{Digest, Sha256};

/// The header carrying the signature.
pub const SIGNATURE_HEADER: &str = "kippu-signature";
/// The header naming the delivery, `<webhook id>:<sequence>`, for deduplication.
pub const DELIVERY_HEADER: &str = "kippu-delivery";
const CONTEXT: &str = "kippu-webhook-v1";

fn message(url: &str, timestamp: i64, body: &[u8]) -> String {
    let digest = hex::encode(Sha256::digest(body));
    format!("{CONTEXT}\n{url}\n{timestamp}\n{digest}")
}

/// The id a webhook key is known by: like ticket keys, the first eight bytes of
/// `SHA-512(public key)`, in hex.
pub fn key_id(key: &VerifyingKey) -> String {
    KeyId::of(key.as_bytes()).to_string()
}

/// The `Kippu-Signature` value for a delivery of `body` to `url`.
pub fn sign(key: &SigningKey, url: &str, timestamp: i64, body: &[u8]) -> String {
    let signature = key.sign(message(url, timestamp, body).as_bytes());
    format!(
        "key={}, ts={timestamp}, sig={}",
        key_id(&key.verifying_key()),
        URL_SAFE_NO_PAD.encode(signature.to_bytes())
    )
}

/// Checks a `Kippu-Signature` value as a receiver would, returning its timestamp. The
/// caller decides how far from its clock the timestamp may be.
pub fn verify(key: &VerifyingKey, header: &str, url: &str, body: &[u8]) -> Option<i64> {
    let (mut key_id_seen, mut timestamp, mut signature) = (None, None, None);
    for field in header.split(',') {
        let (name, value) = field.trim().split_once('=')?;
        match name {
            "key" => key_id_seen = Some(value.to_owned()),
            "ts" => timestamp = value.parse::<i64>().ok(),
            "sig" => {
                let bytes: [u8; 64] = URL_SAFE_NO_PAD.decode(value).ok()?.try_into().ok()?;
                signature = Some(Signature::from_bytes(&bytes));
            }
            _ => {}
        }
    }
    let timestamp = timestamp?;
    (key_id_seen? == key_id(key)).then_some(())?;
    key.verify_strict(message(url, timestamp, body).as_bytes(), &signature?)
        .ok()?;
    Some(timestamp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signatures_bind_the_url_and_the_body() {
        let key = SigningKey::from_bytes(&[9; 32]);
        let public = key.verifying_key();
        let header = sign(&key, "https://hooks.example.org/kippu", 1_000, b"{}");
        assert_eq!(
            verify(&public, &header, "https://hooks.example.org/kippu", b"{}"),
            Some(1_000)
        );
        assert_eq!(
            verify(&public, &header, "https://evil.example/kippu", b"{}"),
            None
        );
        assert_eq!(
            verify(&public, &header, "https://hooks.example.org/kippu", b"[]"),
            None
        );
        let other = SigningKey::from_bytes(&[8; 32]).verifying_key();
        assert_eq!(
            verify(&other, &header, "https://hooks.example.org/kippu", b"{}"),
            None
        );
    }
}
