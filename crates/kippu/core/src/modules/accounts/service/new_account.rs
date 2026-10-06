//! Creating password accounts, shared by signing up and by admins.

use kippu_domain::AccountId;
use kippu_domain::account::{Account, Role};
use kippu_domain::validation::{Email, non_empty};

use crate::app::AppState;
use crate::auth::password::{hash_password, validate_password};
use crate::error::ApiResult;

/// Who an account is and how it signs in with a password.
#[derive(Debug, Clone)]
pub struct NewAccount {
    /// Sign-in address.
    pub email: String,
    /// 10 to 256 characters.
    pub password: String,
    /// Name shown to others.
    pub display_name: String,
}

fn validate_display_name(display_name: &str) -> ApiResult<()> {
    Ok(non_empty("display_name", display_name, 100)?)
}

/// Creates an account with an email and password.
pub(super) async fn insert_account(
    state: &AppState,
    new: NewAccount,
    role: Role,
) -> ApiResult<Account> {
    let email = Email::new(new.email)?;
    validate_password(&new.password)?;
    validate_display_name(&new.display_name)?;
    let account = Account {
        id: AccountId::generate(),
        email: Some(email),
        display_name: new.display_name,
        role,
        created_at: state.now(),
    };
    let password_hash = hash_password(new.password).await?;
    state
        .store()
        .insert_account(&account, Some(&password_hash))
        .await?;
    Ok(account)
}
