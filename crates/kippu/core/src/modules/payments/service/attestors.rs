//! Registering, revoking and re-keying attestors.

use kippu_domain::payment::{Attestor, AttestorKey, Environment};
use kippu_domain::validation::non_empty;
use kippu_domain::{AttestorId, ValidationError};

use crate::app::AppState;
use crate::auth::{Principal, Scope};
use crate::error::{ApiError, ApiResult};
use crate::keys::parse_verifying_key;
use crate::modules::payments::permissions::ATTESTORS_MANAGE;

/// A new attestor and its first keys.
#[derive(Debug, Clone)]
pub struct NewAttestor {
    /// Display name, e.g. "Stripe gateway".
    pub name: String,
    /// Sandbox attestors can only settle sandbox sales.
    pub environment: Environment,
    /// Signing keys.
    pub keys: Vec<NewAttestorKey>,
}

/// A signing key to register for an attestor.
#[derive(Debug, Clone)]
pub struct NewAttestorKey {
    /// The attestor's name for the key, sent in `Kippu-Signature`.
    pub key_id: String,
    /// Ed25519 public key, base64 or SPKI PEM.
    pub public_key: String,
}

/// An attestor and its signing keys.
#[derive(Debug, Clone)]
pub struct AttestorWithKeys {
    /// The attestor.
    pub attestor: Attestor,
    /// Its keys, revoked ones included.
    pub keys: Vec<AttestorKey>,
}

async fn with_keys(state: &AppState, attestor: Attestor) -> ApiResult<AttestorWithKeys> {
    let keys = state.store().attestor_keys(attestor.id).await?;
    Ok(AttestorWithKeys { attestor, keys })
}

fn attestor_key(
    state: &AppState,
    attestor: AttestorId,
    key: &NewAttestorKey,
) -> ApiResult<AttestorKey> {
    non_empty("key_id", &key.key_id, 64)?;
    let public_key = parse_verifying_key("public_key", &key.public_key)
        .map_err(|_| ValidationError::new("public_key", "must be an Ed25519 public key"))?;
    Ok(AttestorKey {
        attestor_id: attestor,
        key_id: key.key_id.clone(),
        public_key: public_key.to_bytes(),
        revoked: false,
        created_at: state.now(),
    })
}

/// Registers an attestor: a service trusted to report payments.
pub async fn create_attestor(
    state: &AppState,
    principal: &Principal,
    new: NewAttestor,
) -> ApiResult<AttestorWithKeys> {
    state.authorize(principal, ATTESTORS_MANAGE, Scope::Global)?;
    non_empty("name", &new.name, 200)?;
    let attestor = Attestor {
        id: AttestorId::generate(),
        name: new.name,
        environment: new.environment,
        revoked: false,
        created_at: state.now(),
    };
    let keys = new
        .keys
        .iter()
        .map(|key| attestor_key(state, attestor.id, key))
        .collect::<ApiResult<Vec<_>>>()?;
    state.store().insert_attestor(&attestor).await?;
    for key in &keys {
        state.store().insert_attestor_key(key).await?;
    }
    state
        .audit(principal, "attestor.create", attestor.id)
        .await?;
    with_keys(state, attestor).await
}

/// Every attestor, including the built-in `free` and `manual` ones.
pub async fn attestors(
    state: &AppState,
    principal: &Principal,
) -> ApiResult<Vec<AttestorWithKeys>> {
    state.authorize(principal, ATTESTORS_MANAGE, Scope::Global)?;
    let mut attestors = Vec::new();
    for attestor in state.store().list_attestors().await? {
        attestors.push(with_keys(state, attestor).await?);
    }
    Ok(attestors)
}

/// Revokes or restores an attestor. Takes effect immediately on every instance.
pub async fn set_attestor_revoked(
    state: &AppState,
    principal: &Principal,
    attestor_id: AttestorId,
    revoked: bool,
) -> ApiResult<()> {
    state.authorize(principal, ATTESTORS_MANAGE, Scope::Global)?;
    if !state
        .store()
        .set_attestor_revoked(attestor_id, revoked)
        .await?
    {
        return Err(ApiError::not_found("attestor"));
    }
    let action = if revoked {
        "attestor.revoke"
    } else {
        "attestor.restore"
    };
    state.audit(principal, action, attestor_id).await
}

/// Registers another signing key for an attestor, e.g. to rotate keys.
pub async fn add_attestor_key(
    state: &AppState,
    principal: &Principal,
    attestor_id: AttestorId,
    key: NewAttestorKey,
) -> ApiResult<()> {
    state.authorize(principal, ATTESTORS_MANAGE, Scope::Global)?;
    let attestor = state
        .store()
        .attestor(attestor_id)
        .await?
        .filter(|attestor| !attestor.id.is_builtin())
        .ok_or_else(|| ApiError::not_found("attestor"))?;
    state
        .store()
        .insert_attestor_key(&attestor_key(state, attestor.id, &key)?)
        .await?;
    state
        .audit(
            principal,
            "attestor.key.add",
            format!("{attestor_id}/{}", key.key_id),
        )
        .await
}

/// Revokes or restores one signing key.
pub async fn set_attestor_key_revoked(
    state: &AppState,
    principal: &Principal,
    attestor_id: AttestorId,
    key_id: &str,
    revoked: bool,
) -> ApiResult<()> {
    state.authorize(principal, ATTESTORS_MANAGE, Scope::Global)?;
    if !state
        .store()
        .set_attestor_key_revoked(attestor_id, key_id, revoked)
        .await?
    {
        return Err(ApiError::not_found("attestor key"));
    }
    state
        .audit(
            principal,
            "attestor.key.revoke",
            format!("{attestor_id}/{key_id}"),
        )
        .await
}
