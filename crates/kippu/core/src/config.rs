//! Configuration. Every value can come from a file, the environment or the command line; the
//! launcher (`kippu-server`) merges those sources into this structure.

use std::net::SocketAddr;

use secrecy::SecretString;
use serde::Deserialize;

/// Everything a Kippu instance needs to know.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// HTTP server settings.
    #[serde(default)]
    pub server: ServerConfig,
    /// The database. Required: Kippu keeps no state of its own.
    pub database: DatabaseConfig,
    /// This deployment's identity.
    #[serde(default)]
    pub issuer: IssuerConfig,
    /// Signing keys.
    pub keys: KeysConfig,
    /// Root credentials.
    #[serde(default)]
    pub root: RootConfig,
    /// Token lifetimes.
    #[serde(default)]
    pub auth: AuthConfig,
    /// Background work.
    #[serde(default)]
    pub workers: WorkersConfig,
}

/// HTTP server settings.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerConfig {
    /// Address to listen on.
    pub listen: SocketAddr,
    /// Requests running longer than this are answered with 408.
    pub request_timeout_seconds: u64,
    /// Largest accepted request body.
    pub max_body_bytes: usize,
    /// Origins allowed to call the API from a browser. Empty disables CORS.
    pub cors_allowed_origins: Vec<String>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            listen: SocketAddr::from(([0, 0, 0, 0], 8080)),
            request_timeout_seconds: 30,
            max_body_bytes: 1024 * 1024,
            cors_allowed_origins: Vec::new(),
        }
    }
}

/// The database.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatabaseConfig {
    /// Connection URL; its scheme selects the adapter, e.g. `sqlite:///var/lib/kippu.db`.
    pub url: SecretString,
    /// Apply schema migrations when the server starts (default). Turn off to run
    /// `kippu migrate` as a separate deployment step instead.
    #[serde(default = "enabled")]
    pub auto_migrate: bool,
}

const fn enabled() -> bool {
    true
}

/// This deployment's identity.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct IssuerConfig {
    /// Written into every ticket's `issuer` claim and required as the `aud` of root tokens,
    /// e.g. `kippu.example.org`.
    pub id: String,
}

impl Default for IssuerConfig {
    fn default() -> Self {
        Self {
            id: "kippu".to_owned(),
        }
    }
}

/// Signing keys. Secret keys are 32-byte Ed25519 seeds, base64-encoded, or PKCS#8 PEM.
/// Generate them with `kippu keygen`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeysConfig {
    /// Signs tickets. Its public key is published at `/.well-known/kippu/ticket-keys`.
    pub ticket_signing_key: SecretString,
    /// Public keys of previous ticket signing keys, still published so verifiers accept
    /// tickets issued before a rotation.
    #[serde(default)]
    pub retired_ticket_keys: Vec<String>,
    /// Signs access tokens, queue tickets and admission passes. Every instance must share it.
    pub token_signing_key: SecretString,
}

/// Root credentials. Root has no password and no database record: it is whoever holds the
/// private key matching one of these public keys.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RootConfig {
    /// Trusted root public keys.
    pub keys: Vec<RootKey>,
    /// Longest lifetime a root token may claim.
    pub max_token_ttl_seconds: u32,
}

impl Default for RootConfig {
    fn default() -> Self {
        Self {
            keys: Vec::new(),
            max_token_ttl_seconds: 600,
        }
    }
}

/// One root public key.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootKey {
    /// Name of the key, used as the token's `kid` (`root:<name>`) and in the audit log.
    pub name: String,
    /// Ed25519 public key, base64 or SPKI PEM.
    pub public_key: String,
}

/// Token lifetimes.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AuthConfig {
    /// Lifetime of access tokens.
    pub access_token_ttl_seconds: u32,
    /// Lifetime of refresh tokens (sessions).
    pub refresh_token_ttl_seconds: u32,
    /// How long a waiting-room queue ticket stays usable.
    pub queue_ticket_ttl_seconds: u32,
    /// How long an admission pass lets its holder submit purchase requests.
    pub admission_pass_ttl_seconds: u32,
    /// Link an external sign-in to the existing account with the same email, when the
    /// provider verified that email. Off by default: Kippu does not verify the emails people
    /// register with, so whoever registered an address first would gain the provider's
    /// sign-in for it. Turn on only if every account's email is known to be verified.
    pub link_by_verified_email: bool,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            access_token_ttl_seconds: 15 * 60,
            refresh_token_ttl_seconds: 30 * 24 * 60 * 60,
            queue_ticket_ttl_seconds: 6 * 60 * 60,
            admission_pass_ttl_seconds: 10 * 60,
            link_by_verified_email: false,
        }
    }
}

/// Background work. Every task is safe to run on any number of instances at once.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WorkersConfig {
    /// Run background tasks in this process. Disable to serve HTTP only.
    pub enabled: bool,
    /// Purchase requests claimed per batch.
    pub purchase_batch_size: u32,
    /// Pause between purchase batches when the queue is empty.
    pub purchase_interval_ms: u64,
    /// How long a claimed purchase request is reserved for the claiming worker.
    pub purchase_lease_seconds: u32,
    /// How many sales one batch processes at once, when the database allows concurrent
    /// writers (PostgreSQL, MySQL). Requests of the same sale are always processed in order.
    pub purchase_concurrency: u32,
    /// How often overdue reservations are expired.
    pub expiry_interval_ms: u64,
    /// How often waiting rooms admit a batch.
    pub admission_interval_ms: u64,
}

impl Default for WorkersConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            purchase_batch_size: 50,
            purchase_interval_ms: 200,
            purchase_lease_seconds: 30,
            purchase_concurrency: 8,
            expiry_interval_ms: 1_000,
            admission_interval_ms: 1_000,
        }
    }
}
