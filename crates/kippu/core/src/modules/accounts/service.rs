//! Account use cases, independent of HTTP.

use kippu_domain::AccountId;
use kippu_domain::validation::non_empty;

use crate::app::AppState;
use crate::auth::password::{hash_password, validate_password, verify_password};
use crate::error::{ApiError, ApiResult};

pub(crate) fn validate_display_name(display_name: &str) -> ApiResult<()> {
    Ok(non_empty("display_name", display_name, 100)?)
}

/// Sets or changes an account's password. Changing one requires the current password;
/// accounts created through an external sign-in set their first one without.
pub(crate) async fn set_password(
    state: &AppState,
    account: AccountId,
    current: Option<String>,
    new: String,
) -> ApiResult<()> {
    validate_password(&new)?;
    if let Some(hash) = state.store().password_hash(account).await? {
        let matches = match current {
            Some(current) => verify_password(current, hash).await?,
            None => false,
        };
        if !matches {
            return Err(ApiError::new(
                axum::http::StatusCode::FORBIDDEN,
                "wrong-password",
                "the current password is missing or wrong",
            ));
        }
    }
    let hash = hash_password(new).await?;
    if state.store().set_password_hash(account, &hash).await? {
        Ok(())
    } else {
        Err(ApiError::not_found("account"))
    }
}
