use std::fmt;
use std::ops::{Add, Sub};

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

pub use time::Duration;

/// A UTC instant with microsecond precision.
///
/// Microseconds are what every supported database stores losslessly, so a timestamp always
/// reads back exactly as it was written. Serialized as RFC 3339.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(OffsetDateTime);

impl Timestamp {
    /// The Unix epoch.
    pub const UNIX_EPOCH: Self = Self(OffsetDateTime::UNIX_EPOCH);

    /// Converts from microseconds since the Unix epoch. Out-of-range values saturate.
    pub fn from_unix_micros(micros: i64) -> Self {
        let nanos = i128::from(micros) * 1_000;
        OffsetDateTime::from_unix_timestamp_nanos(nanos)
            .map_or_else(|_| if micros < 0 { Self::MIN } else { Self::MAX }, Self)
    }

    /// Converts from seconds since the Unix epoch. Out-of-range values saturate.
    pub fn from_unix_seconds(seconds: i64) -> Self {
        Self::from_unix_micros(seconds.saturating_mul(1_000_000))
    }

    /// Converts from any instant, truncating to microseconds.
    pub fn from_offset_date_time(instant: OffsetDateTime) -> Self {
        let micros = instant.unix_timestamp_nanos() / 1_000;
        Self::from_unix_micros(i64::try_from(micros).unwrap_or(i64::MAX))
    }

    /// Microseconds since the Unix epoch.
    pub fn unix_micros(self) -> i64 {
        i64::try_from(self.0.unix_timestamp_nanos() / 1_000).unwrap_or(i64::MAX)
    }

    /// Whole seconds since the Unix epoch.
    pub fn unix_seconds(self) -> i64 {
        self.0.unix_timestamp()
    }

    /// The instant as a `time::OffsetDateTime` in UTC.
    pub const fn to_offset_date_time(self) -> OffsetDateTime {
        self.0
    }

    /// Adds a duration, saturating at the representable range.
    #[must_use]
    pub fn saturating_add(self, duration: Duration) -> Self {
        Self::from_offset_date_time(self.0.saturating_add(duration))
    }

    const MIN: Self = Self(OffsetDateTime::UNIX_EPOCH.saturating_sub(Duration::MAX));
    const MAX: Self = Self(OffsetDateTime::UNIX_EPOCH.saturating_add(Duration::MAX));
}

impl Add<Duration> for Timestamp {
    type Output = Self;

    fn add(self, duration: Duration) -> Self {
        self.saturating_add(duration)
    }
}

impl Sub for Timestamp {
    type Output = Duration;

    fn sub(self, earlier: Self) -> Duration {
        self.0 - earlier.0
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0.format(&Rfc3339) {
            Ok(text) => f.write_str(&text),
            Err(_) => write!(f, "@{}µs", self.unix_micros()),
        }
    }
}

impl fmt::Debug for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Timestamp({self})")
    }
}

impl Serialize for Timestamp {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let text = self.0.format(&Rfc3339).map_err(serde::ser::Error::custom)?;
        serializer.serialize_str(&text)
    }
}

impl<'de> Deserialize<'de> for Timestamp {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        let instant = OffsetDateTime::parse(&text, &Rfc3339).map_err(serde::de::Error::custom)?;
        Ok(Self::from_offset_date_time(instant))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn microseconds_round_trip() {
        let instant = Timestamp::from_unix_micros(1_767_225_600_123_456);
        assert_eq!(instant.unix_micros(), 1_767_225_600_123_456);
        assert_eq!(instant.unix_seconds(), 1_767_225_600);
    }

    #[test]
    fn serializes_as_rfc_3339() {
        let instant = Timestamp::from_unix_seconds(1_767_225_600);
        let json = serde_json::to_string(&instant).unwrap();
        assert_eq!(json, "\"2026-01-01T00:00:00Z\"");
        assert_eq!(serde_json::from_str::<Timestamp>(&json).unwrap(), instant);
    }

    #[test]
    fn parsing_truncates_to_microseconds() {
        let parsed: Timestamp =
            serde_json::from_str("\"2026-01-01T09:00:00.123456789+09:00\"").unwrap();
        assert_eq!(parsed.unix_micros(), 1_767_225_600_123_456);
    }
}
