//! Money as integer minor units. Never floating point.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::ValidationError;

/// An ISO 4217 currency code, such as `JPY` or `CNY`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema), schema(value_type = String, example = "JPY"))]
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

/// An amount of money in the currency's minor unit (yen, fen, cents). Never negative: the
/// fields are private, so every amount, deserialized ones included, goes through
/// [`Money::new`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "MoneyFields")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Money {
    /// Amount in minor units. Never negative.
    amount_minor: i64,
    /// Currency of the amount.
    currency: Currency,
}

/// The fields of [`Money`] as they arrive, before they are checked.
#[derive(Deserialize)]
struct MoneyFields {
    amount_minor: i64,
    currency: Currency,
}

impl TryFrom<MoneyFields> for Money {
    type Error = ValidationError;

    fn try_from(fields: MoneyFields) -> Result<Self, Self::Error> {
        Self::new(fields.amount_minor, fields.currency)
    }
}

/// Why amounts could not be added or multiplied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MoneyError {
    /// The amounts are in different currencies.
    #[error("amounts in different currencies")]
    CurrencyMismatch,
    /// The result does not fit in 63 bits.
    #[error("amount overflows")]
    Overflow,
    /// There were no amounts to add up, so the sum has no currency.
    #[error("no amounts to add up")]
    Empty,
}

impl Money {
    /// Creates an amount, rejecting negative values.
    pub const fn new(amount_minor: i64, currency: Currency) -> Result<Self, ValidationError> {
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

    /// Amount in minor units; never negative.
    pub const fn amount_minor(&self) -> i64 {
        self.amount_minor
    }

    /// Currency of the amount.
    pub const fn currency(&self) -> Currency {
        self.currency
    }

    /// Whether the amount is zero, i.e. free.
    pub const fn is_zero(&self) -> bool {
        self.amount_minor == 0
    }

    /// Adds two amounts of the same currency.
    pub fn try_add(self, other: Self) -> Result<Self, MoneyError> {
        if self.currency != other.currency {
            return Err(MoneyError::CurrencyMismatch);
        }
        Ok(Self {
            amount_minor: self
                .amount_minor
                .checked_add(other.amount_minor)
                .ok_or(MoneyError::Overflow)?,
            currency: self.currency,
        })
    }

    /// Multiplies by a quantity.
    pub fn try_mul(self, quantity: u32) -> Result<Self, MoneyError> {
        Ok(Self {
            amount_minor: self
                .amount_minor
                .checked_mul(i64::from(quantity))
                .ok_or(MoneyError::Overflow)?,
            currency: self.currency,
        })
    }

    /// Adds up amounts of one currency. The first failure ends the sum.
    pub fn try_sum(
        amounts: impl IntoIterator<Item = Result<Self, MoneyError>>,
    ) -> Result<Self, MoneyError> {
        let mut amounts = amounts.into_iter();
        let first = amounts.next().ok_or(MoneyError::Empty)??;
        amounts.try_fold(first, |sum, amount| sum.try_add(amount?))
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
        assert_eq!(yen.try_add(yen).unwrap().amount_minor(), 2_000);
        assert_eq!(yen.try_add(yuan), Err(MoneyError::CurrencyMismatch));
        assert_eq!(yen.try_mul(3).unwrap().amount_minor(), 3_000);
        let most = Money::new(i64::MAX, Currency::JPY).unwrap();
        assert_eq!(most.try_mul(2), Err(MoneyError::Overflow));
        assert!(Money::new(-1, Currency::JPY).is_err());
    }

    #[test]
    fn sums_stop_at_the_first_failure() {
        let yen = |amount| Ok(Money::new(amount, Currency::JPY).unwrap());
        let yuan = Ok(Money::new(1, "CNY".parse().unwrap()).unwrap());
        assert_eq!(Money::try_sum([yen(1), yen(2)]).unwrap().amount_minor(), 3);
        assert_eq!(
            Money::try_sum([yen(1), yuan, yuan]),
            Err(MoneyError::CurrencyMismatch)
        );
        assert_eq!(
            Money::try_sum([yen(i64::MAX), yen(1), yen(1)]),
            Err(MoneyError::Overflow)
        );
        assert_eq!(Money::try_sum([]), Err(MoneyError::Empty));
    }

    #[test]
    fn negative_amounts_cannot_be_deserialized() {
        let negative = serde_json::from_str::<Money>(r#"{"amount_minor":-1,"currency":"JPY"}"#);
        assert!(negative.is_err());
        let money: Money = serde_json::from_str(r#"{"amount_minor":5,"currency":"JPY"}"#).unwrap();
        assert_eq!(money, Money::new(5, Currency::JPY).unwrap());
    }

    #[test]
    fn serializes_with_the_currency_code() {
        let json = serde_json::to_string(&Money::new(1_200, Currency::JPY).unwrap()).unwrap();
        assert_eq!(json, r#"{"amount_minor":1200,"currency":"JPY"}"#);
    }
}
