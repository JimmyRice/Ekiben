use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::service::{NewWebhook, SigningKey, WebhookChanges};
use crate::keys::encode_key;

/// A new webhook. It receives events from now on, not those recorded before.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateWebhookRequest {
    /// Where to POST events: `https`, and not a loopback or private address, unless the
    /// deployment allows otherwise.
    pub url: String,
    /// Topics to receive, e.g. `["tickets.issued"]`; empty or absent means all of them.
    #[serde(default)]
    pub topics: Vec<String>,
}

/// Changes to a webhook: only the fields to change, plus the `version` you last read.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct WebhookPatch {
    /// The version you are editing; a mismatch fails with 412.
    pub version: i64,
    /// Where to POST events.
    pub url: Option<String>,
    /// Topics to receive, replaced as a whole; empty means all of them.
    pub topics: Option<Vec<String>>,
    /// Pause (`false`) or resume (`true`) delivery. A paused webhook resumes where it stopped.
    pub active: Option<bool>,
}

/// The key webhook deliveries are signed with.
#[derive(Debug, Serialize, ToSchema)]
pub struct WebhookKey {
    /// The id in `Kippu-Signature`'s `key=`.
    pub key_id: String,
    /// Always `"Ed25519"`.
    pub algorithm: &'static str,
    /// The 32-byte public key, base64.
    pub public_key: String,
}

/// The keys webhook receivers should trust.
#[derive(Debug, Serialize, ToSchema)]
pub struct WebhookKeys {
    /// Currently one key; receivers should look keys up by id so rotation needs no change.
    pub keys: Vec<WebhookKey>,
}

impl From<CreateWebhookRequest> for NewWebhook {
    fn from(request: CreateWebhookRequest) -> Self {
        Self {
            url: request.url,
            topics: request.topics,
        }
    }
}

impl From<WebhookPatch> for WebhookChanges {
    fn from(patch: WebhookPatch) -> Self {
        Self {
            url: patch.url,
            topics: patch.topics,
            active: patch.active,
        }
    }
}

impl From<SigningKey> for WebhookKey {
    fn from(key: SigningKey) -> Self {
        Self {
            key_id: key.key_id,
            algorithm: "Ed25519",
            public_key: encode_key(&key.public_key),
        }
    }
}
