//! A minimal JSON Web Token codec that only speaks EdDSA (RFC 7515 + RFC 8037).
//!
//! Accepting a single algorithm makes algorithm-confusion attacks impossible by construction,
//! and keeps the dependency tree to `ed25519-dalek`. The tokens are ordinary JWTs, so any JWT
//! library can mint a root token.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

const ALGORITHM: &str = "EdDSA";

/// Why a token was rejected. Deliberately vague towards clients.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum JwtError {
    /// Not three base64url segments of valid JSON.
    #[error("malformed token")]
    Malformed,
    /// The header names an algorithm other than EdDSA.
    #[error("unsupported algorithm")]
    UnsupportedAlgorithm,
    /// The signature does not verify.
    #[error("bad signature")]
    BadSignature,
}

/// The JOSE header.
#[derive(Debug, Serialize, Deserialize)]
pub struct Header {
    /// Signature algorithm. Must be `EdDSA`.
    pub alg: String,
    /// Key id, telling the verifier which key to use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kid: Option<String>,
    /// Media type, `JWT`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub typ: Option<String>,
}

/// Signs `claims` into a compact JWT.
pub fn sign<C: Serialize>(key: &SigningKey, kid: &str, claims: &C) -> String {
    let header = Header {
        alg: ALGORITHM.to_owned(),
        kid: Some(kid.to_owned()),
        typ: Some("JWT".to_owned()),
    };
    // Serializing plain structs of strings and numbers cannot fail.
    let header = serde_json::to_vec(&header).unwrap_or_default();
    let claims = serde_json::to_vec(claims).unwrap_or_default();
    let signing_input = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(header),
        URL_SAFE_NO_PAD.encode(claims)
    );
    let signature = key.sign(signing_input.as_bytes());
    format!(
        "{signing_input}.{}",
        URL_SAFE_NO_PAD.encode(signature.to_bytes())
    )
}

/// A token split into its parts, header decoded, signature not yet checked.
pub struct Unverified<'t> {
    /// The decoded header, for choosing a key.
    pub header: Header,
    signing_input: &'t str,
    claims: &'t str,
    signature: &'t str,
}

/// Splits a token and decodes its header.
pub fn parse(token: &str) -> Result<Unverified<'_>, JwtError> {
    let (signing_input, signature) = token.rsplit_once('.').ok_or(JwtError::Malformed)?;
    let (header, claims) = signing_input.split_once('.').ok_or(JwtError::Malformed)?;
    let header: Header = decode_json(header)?;
    if header.alg != ALGORITHM {
        return Err(JwtError::UnsupportedAlgorithm);
    }
    Ok(Unverified {
        header,
        signing_input,
        claims,
        signature,
    })
}

impl Unverified<'_> {
    /// Checks the signature with `key` and decodes the claims.
    pub fn verify<C: DeserializeOwned>(&self, key: &VerifyingKey) -> Result<C, JwtError> {
        let signature = URL_SAFE_NO_PAD
            .decode(self.signature)
            .map_err(|_| JwtError::Malformed)?;
        let signature: [u8; 64] = signature.try_into().map_err(|_| JwtError::Malformed)?;
        key.verify_strict(
            self.signing_input.as_bytes(),
            &Signature::from_bytes(&signature),
        )
        .map_err(|_| JwtError::BadSignature)?;
        decode_json(self.claims)
    }
}

fn decode_json<T: DeserializeOwned>(segment: &str) -> Result<T, JwtError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(segment)
        .map_err(|_| JwtError::Malformed)?;
    serde_json::from_slice(&bytes).map_err(|_| JwtError::Malformed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Claims {
        sub: String,
    }

    #[test]
    fn round_trips_and_rejects_tampering() {
        let key = SigningKey::from_bytes(&[3; 32]);
        let token = sign(&key, "test", &Claims { sub: "miku".into() });
        let parsed = parse(&token).unwrap();
        assert_eq!(parsed.header.kid.as_deref(), Some("test"));
        assert_eq!(
            parsed.verify::<Claims>(&key.verifying_key()).unwrap().sub,
            "miku"
        );

        let other = SigningKey::from_bytes(&[4; 32]).verifying_key();
        assert_eq!(
            parse(&token).unwrap().verify::<Claims>(&other),
            Err(JwtError::BadSignature)
        );

        let forged_claims = URL_SAFE_NO_PAD.encode(br#"{"sub":"root"}"#);
        let mut parts: Vec<&str> = token.split('.').collect();
        parts[1] = &forged_claims;
        let forged = parts.join(".");
        assert_eq!(
            parse(&forged)
                .unwrap()
                .verify::<Claims>(&key.verifying_key()),
            Err(JwtError::BadSignature)
        );
    }

    #[test]
    fn rejects_other_algorithms() {
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"none"}"#);
        let token = format!("{header}.e30.");
        assert!(matches!(parse(&token), Err(JwtError::UnsupportedAlgorithm)));
    }
}
