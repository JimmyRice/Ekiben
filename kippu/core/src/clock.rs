//! Where the current time comes from. Injected, so tests can control it.

use std::sync::atomic::{AtomicI64, Ordering};

use kippu_domain::{Duration, Timestamp};

/// A source of the current time.
pub trait Clock: Send + Sync + 'static {
    /// The current instant.
    fn now(&self) -> Timestamp;
}

/// The system's real-time clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        Timestamp::from_offset_date_time(time::OffsetDateTime::now_utc())
    }
}

/// A clock that only moves when told to. For tests.
#[derive(Debug)]
pub struct ManualClock {
    micros: AtomicI64,
}

impl ManualClock {
    /// A clock stopped at `start`.
    pub fn new(start: Timestamp) -> Self {
        Self {
            micros: AtomicI64::new(start.unix_micros()),
        }
    }

    /// Moves the clock forward.
    pub fn advance(&self, by: Duration) {
        let micros = i64::try_from(by.whole_microseconds()).unwrap_or(i64::MAX);
        self.micros.fetch_add(micros, Ordering::SeqCst);
    }

    /// Sets the clock to `instant`.
    pub fn set(&self, instant: Timestamp) {
        self.micros.store(instant.unix_micros(), Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Timestamp {
        Timestamp::from_unix_micros(self.micros.load(Ordering::SeqCst))
    }
}
