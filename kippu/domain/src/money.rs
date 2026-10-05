//! Money as integer minor units. Never floating point.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::ValidationError;

/// An ISO 4217 currency code, such as `JPY` or `CNY`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Currency([u8; 3]);

impl Currency {
    /// Japanese yen.
    pub const JPY: Self = Self(*b"JPY");

    /// The three-letter code.
    pub fn as_str(&self) -> &str {
        // Construction guarantees three ASCII uppercase letters.
        std::str::from_utf8(&self.0).unwrap_or("???")
    }
}

impl FromStr for Currency {
    type Err = ValidationError;

    fn from_str(code: &str) -> Result<Self, Self::Err> {
        let bytes: [u8; 3] = code.as_bytes().try_into().map_err(|_| {
            ValidationError::new("currency", "must be a three-letter ISO 4217 code")
        })?;
        if !bytes.iter().all(u8::is_ascii_uppercase) {
            return Err(ValidationError::new(
                "currency",
                "must be three uppercase letters",
            ));
        }
        Ok(Self(bytes))
    }
}

impl TryFrom<String> for Currency {
    type Error = ValidationError;

    fn try_from(code: String) -> Result<Self, Self::Error> {
        code.parse()
    }
}

impl From<Currency> for String {
    fn from(currency: Currency) -> Self {
        currency.as_str().to_owned()
    }
}

impl fmt::Display for Currency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Debug for Currency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Currency({self})")
    }
}

/// An amount of money in the currency's minor unit (yen, fen, cents).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Money {
    /// Amount in minor units. Never negative.
    pub amount_minor: i64,
    /// Currency of the amount.
    pub currency: Currency,
}

impl Money {
    /// Creates an amount, rejecting negative values.
    pub fn new(amount_minor: i64, currency: Currency) -> Result<Self, ValidationError> {
        if amount_minor < 0 {
            return Err(ValidationError::new("amount_minor", "must not be negative"));
        }
        Ok(Self {
            amount_minor,
            currency,
        })
    }

    /// Zero in the given currency.
    pub const fn zero(currency: Currency) -> Self {
        Self {
            amount_minor: 0,
            currency,
        }
    }

    /// Whether the amount is zero, i.e. free.
    pub const fn is_zero(&self) -> bool {
        self.amount_minor == 0
    }

    /// Adds two amounts of the same currency.
    pub fn checked_add(self, other: Self) -> Option<Self> {
        (self.currency == other.currency).then_some(())?;
        Some(Self {
            amount_minor: self.amount_minor.checked_add(other.amount_minor)?,
            currency: self.currency,
        })
    }

    /// Multiplies by a quantity.
    pub fn checked_mul(self, quantity: u32) -> Option<Self> {
        Some(Self {
            amount_minor: self.amount_minor.checked_mul(i64::from(quantity))?,
            currency: self.currency,
        })
    }
}

impl fmt::Display for Money {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.amount_minor, self.currency)
    }
}

impl fmt::Debug for Money {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Money({self})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn currencies_are_three_uppercase_letters() {
        assert_eq!("JPY".parse::<Currency>().unwrap(), Currency::JPY);
        assert!("jpy".parse::<Currency>().is_err());
        assert!("JP".parse::<Currency>().is_err());
        assert!("JPYY".parse::<Currency>().is_err());
    }

    #[test]
    fn arithmetic_refuses_mixed_currencies_and_overflow() {
        let yen = Money::new(1_000, Currency::JPY).unwrap();
        let yuan = Money::new(1_000, "CNY".parse().unwrap()).unwrap();
        assert_eq!(yen.checked_add(yen).unwrap().amount_minor, 2_000);
        assert_eq!(yen.checked_add(yuan), None);
        assert_eq!(yen.checked_mul(3).unwrap().amount_minor, 3_000);
        assert_eq!(
            Money::new(i64::MAX, Currency::JPY).unwrap().checked_mul(2),
            None
        );
        assert!(Money::new(-1, Currency::JPY).is_err());
    }

    #[test]
    fn serializes_with_the_currency_code() {
        let json = serde_json::to_string(&Money::new(1_200, Currency::JPY).unwrap()).unwrap();
        assert_eq!(json, r#"{"amount_minor":1200,"currency":"JPY"}"#);
    }
}
