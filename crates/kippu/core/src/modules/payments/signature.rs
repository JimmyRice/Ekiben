//! Requests signed by an attestor.
//!
//! An attestor signs every request with one of its registered Ed25519 keys:
//!
//! ```text
//! Kippu-Signature: attestor=<attestor id>, key=<key id>, ts=<unix seconds>, sig=<base64url>
//!
//! signed message = "kippu-attestor-v1" \n METHOD \n path?query \n ts \n hex(SHA-256(body))
//! ```
//!
//! The timestamp must be within five minutes of the server's clock. Replaying a signed request
//! within that window is harmless: every attestation is idempotent by its id.

use axum::body::Bytes;
use axum::extract::{FromRequest, Request};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use kippu_domain::AttestorId;
use kippu_domain::payment::Attestor;
use sha2::{Digest, Sha256};

use crate::app::AppState;
use crate::error::{ApiError, ApiResult};

/// The request header carrying the signature.
pub const SIGNATURE_HEADER: &str = "kippu-signature";
const CONTEXT: &str = "kippu-attestor-v1";
const MAX_CLOCK_SKEW_SECONDS: i64 = 300;

/// The bytes an attestor signs.
fn message(method: &str, path_and_query: &str, timestamp: i64, body: &[u8]) -> String {
    let digest = hex::encode(Sha256::digest(body));
    format!("{CONTEXT}\n{method}\n{path_and_query}\n{timestamp}\n{digest}")
}

/// Produces a `Kippu-Signature` header value. Attestor services (and tests) use this.
pub fn sign_request(
    key: &SigningKey,
    attestor: AttestorId,
    key_id: &str,
    method: &str,
    path_and_query: &str,
    timestamp: i64,
    body: &[u8],
) -> String {
    let signature = key.sign(message(method, path_and_query, timestamp, body).as_bytes());
    format!(
        "attestor={attestor}, key={key_id}, ts={timestamp}, sig={}",
        URL_SAFE_NO_PAD.encode(signature.to_bytes())
    )
}

struct Header {
    attestor: AttestorId,
    key_id: String,
    timestamp: i64,
    signature: Signature,
}

fn parse_header(value: &str) -> Option<Header> {
    let (mut attestor, mut key_id, mut timestamp, mut signature) = (None, None, None, None);
    for field in value.split(',') {
        let (name, value) = field.trim().split_once('=')?;
        match name {
            "attestor" => attestor = value.parse().ok(),
            "key" => key_id = Some(value.to_owned()),
            "ts" => timestamp = value.parse().ok(),
            "sig" => {
                let bytes: [u8; 64] = URL_SAFE_NO_PAD.decode(value).ok()?.try_into().ok()?;
                signature = Some(Signature::from_bytes(&bytes));
            }
            _ => {}
        }
    }
    Some(Header {
        attestor: attestor?,
        key_id: key_id?,
        timestamp: timestamp?,
        signature: signature?,
    })
}

/// A request body proven to come from a registered, unrevoked attestor.
#[derive(Debug)]
pub struct Attested {
    /// Who signed it.
    pub attestor: Attestor,
    /// The raw body that was signed.
    pub body: Bytes,
}

impl Attested {
    /// Parses the signed body as JSON.
    pub fn json<T: serde::de::DeserializeOwned>(&self) -> ApiResult<T> {
        serde_json::from_slice(&self.body).map_err(|error| {
            ApiError::new(
                axum::http::StatusCode::BAD_REQUEST,
                "invalid-json",
                error.to_string(),
            )
        })
    }
}

impl FromRequest<AppState> for Attested {
    type Rejection = ApiError;

    async fn from_request(request: Request, state: &AppState) -> Result<Self, ApiError> {
        let rejected = || ApiError::unauthenticated("missing or invalid Kippu-Signature");
        let header = request
            .headers()
            .get(SIGNATURE_HEADER)
            .and_then(|value| value.to_str().ok())
            .and_then(parse_header)
            .ok_or_else(rejected)?;
        let method = request.method().as_str().to_owned();
        let path = request
            .uri()
            .path_and_query()
            .map_or("/", |path| path.as_str())
            .to_owned();
        let body = Bytes::from_request(request, state)
            .await
            .map_err(|_| rejected())?;

        if (state.now().unix_seconds() - header.timestamp).abs() > MAX_CLOCK_SKEW_SECONDS {
            return Err(ApiError::unauthenticated(
                "Kippu-Signature timestamp is too far from now",
            ));
        }
        let attestor = state
            .store()
            .attestor(header.attestor)
            .await?
            .filter(|attestor| !attestor.revoked && !attestor.id.is_builtin())
            .ok_or_else(rejected)?;
        let key = state
            .store()
            .attestor_key(attestor.id, &header.key_id)
            .await?
            .filter(|key| !key.revoked)
            .ok_or_else(rejected)?;
        let verifying_key = VerifyingKey::from_bytes(&key.public_key).map_err(|_| rejected())?;
        verifying_key
            .verify_strict(
                message(&method, &path, header.timestamp, &body).as_bytes(),
                &header.signature,
            )
            .map_err(|_| rejected())?;
        crate::http::trace::record_attestor(attestor.id);
        Ok(Self { attestor, body })
    }
}
