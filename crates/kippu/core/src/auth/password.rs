//! Password hashing with Argon2id.

use std::sync::LazyLock;

use argon2::{Argon2, PasswordHasher, PasswordVerifier};
use kippu_domain::ValidationError;

use crate::error::{ApiError, ApiResult};

/// Checks a new password's length. Length is what matters; composition rules do not help.
pub fn validate_password(password: &str) -> Result<(), ValidationError> {
    let length = password.chars().count();
    if (10..=256).contains(&length) {
        Ok(())
    } else {
        Err(ValidationError::new(
            "password",
            "must be 10 to 256 characters",
        ))
    }
}

/// Hashes a password. Runs on the blocking pool: Argon2 is deliberately slow.
pub async fn hash_password(password: String) -> ApiResult<String> {
    tokio::task::spawn_blocking(move || {
        Argon2::default()
            .hash_password(password.as_bytes())
            .map(|hash| hash.to_string())
            .map_err(|error| ApiError::internal(error.to_string()))
    })
    .await
    .map_err(ApiError::internal)?
}

/// Checks a password against a stored hash.
pub async fn verify_password(password: String, hash: String) -> ApiResult<bool> {
    tokio::task::spawn_blocking(move || {
        Argon2::default()
            .verify_password(password.as_bytes(), hash.as_str())
            .is_ok()
    })
    .await
    .map_err(ApiError::internal)
}

/// A hash to verify against when the account does not exist, so that "no such account" and
/// "wrong password" take the same time and cannot be told apart.
pub static DUMMY_HASH: LazyLock<String> = LazyLock::new(|| {
    Argon2::default()
        .hash_password(b"kippu-dummy-password")
        .map(|hash| hash.to_string())
        .unwrap_or_default()
});
