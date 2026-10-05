//! Validated value types and the error they share.

use std::fmt;

use serde::{Deserialize, Serialize};

/// A value failed validation. `field` names the offending input for API error responses.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{field} {reason}")]
pub struct ValidationError {
    /// The input field that is invalid.
    pub field: &'static str,
    /// Why it is invalid, phrased to follow the field name.
    pub reason: &'static str,
}

impl ValidationError {
    /// Creates a validation error.
    pub const fn new(field: &'static str, reason: &'static str) -> Self {
        Self { field, reason }
    }
}

macro_rules! validated_string {
    ($(#[$meta:meta])* $name:ident, $validate:expr) => {
        $(#[$meta])*
        #[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            /// Validates and wraps a string.
            pub fn new(value: impl Into<String>) -> Result<Self, ValidationError> {
                let validate: fn(String) -> Result<String, ValidationError> = $validate;
                validate(value.into()).map(Self)
            }

            /// The validated string.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = ValidationError;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!(stringify!($name), "({:?})"), self.0)
            }
        }
    };
}

validated_string!(
    /// An email address, trimmed and lower-cased. Only the shape is checked; ownership is not.
    Email,
    |value| {
        let email = value.trim().to_lowercase();
        let valid = email.len() <= 254
            && email
                .split_once('@')
                .is_some_and(|(local, domain)| !local.is_empty() && domain.contains('.'));
        if valid {
            Ok(email)
        } else {
            Err(ValidationError::new("email", "is not a valid email address"))
        }
    }
);

validated_string!(
    /// A URL-friendly name: 1–64 characters of `a-z`, `0-9` and inner hyphens.
    Slug,
    |value| {
        let valid = (1..=64).contains(&value.len())
            && value
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            && !value.starts_with('-')
            && !value.ends_with('-');
        if valid {
            Ok(value)
        } else {
            Err(ValidationError::new("slug", "must be 1-64 characters of a-z, 0-9 and inner hyphens"))
        }
    }
);

validated_string!(
    /// A client-chosen key that makes a retried request safe: 1–255 visible ASCII characters.
    IdempotencyKey,
    |value| {
        let valid = (1..=255).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_graphic());
        if valid {
            Ok(value)
        } else {
            Err(ValidationError::new("Idempotency-Key", "must be 1-255 visible ASCII characters"))
        }
    }
);

/// Validates a required, human-readable text field.
pub fn non_empty(field: &'static str, value: &str, max_len: usize) -> Result<(), ValidationError> {
    if value.trim().is_empty() {
        Err(ValidationError::new(field, "must not be empty"))
    } else if value.chars().count() > max_len {
        Err(ValidationError::new(field, "is too long"))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emails_are_normalised() {
        assert_eq!(
            Email::new("  Miku@Example.ORG ").unwrap().as_str(),
            "miku@example.org"
        );
        assert!(Email::new("no-at-sign").is_err());
        assert!(Email::new("@example.org").is_err());
        assert!(Email::new("miku@localhost").is_err());
    }

    #[test]
    fn slugs_are_url_friendly() {
        assert!(Slug::new("comiket-107").is_ok());
        assert!(Slug::new("Comiket").is_err());
        assert!(Slug::new("-comiket").is_err());
        assert!(Slug::new("").is_err());
    }

    #[test]
    fn idempotency_keys_are_visible_ascii() {
        assert!(IdempotencyKey::new("8e0f9a1c-retry-1").is_ok());
        assert!(IdempotencyKey::new("has space").is_err());
        assert!(IdempotencyKey::new("x".repeat(256)).is_err());
    }
}
