//! Admission control: who may submit a purchase request, and when.
//!
//! A waiting room hands out queue positions in arrival order and, once the sale opens, admits
//! them in batches. A batch is released only while the purchase backlog is small, so
//! admissions follow the speed at which workers actually process purchases — the queue
//! applies back-pressure all the way to the buyer's browser.

use serde::{Deserialize, Serialize};

use crate::validation::ValidationError;

/// How buyers are let into a sale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum AdmissionPolicy {
    /// Anyone may submit purchase requests while the sale is open.
    Open,
    /// Buyers join a queue, also before the sale opens, and are admitted in batches from the
    /// moment it opens.
    WaitingRoom {
        /// How many queue positions are admitted per admission tick, at most: a tick never
        /// admits more people than the sale has tickets left, and a sold-out sale admits
        /// nobody until tickets come back. One sale is served by one transaction at a time, so
        /// about 80% of what the workers serve of a single sale per tick keeps them busy
        /// without the backlog growing.
        admit_per_tick: u32,
        /// Admission pauses while this many purchase requests are waiting to be processed:
        /// about one second of what the workers serve of a single sale, so a buyer let in waits
        /// about a second for the outcome.
        max_backlog: u32,
    },
}

impl AdmissionPolicy {
    /// Checks the policy's parameters.
    pub fn validate(&self) -> Result<(), ValidationError> {
        match *self {
            Self::Open => Ok(()),
            Self::WaitingRoom {
                admit_per_tick,
                max_backlog,
            } => {
                if admit_per_tick == 0 {
                    Err(ValidationError::new(
                        "admission.admit_per_tick",
                        "must be positive",
                    ))
                } else if max_backlog == 0 {
                    Err(ValidationError::new(
                        "admission.max_backlog",
                        "must be positive",
                    ))
                } else {
                    Ok(())
                }
            }
        }
    }
}

/// Progress of a sale's waiting room.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct WaitingRoom {
    /// Highest queue position handed out (or skipped) so far; positions start at 1.
    pub last_position: u64,
    /// Every position up to and including this one has been admitted.
    pub admitted_through: u64,
}

impl WaitingRoom {
    /// The `admitted_through` mark after one admission tick, given how many purchase
    /// requests are still waiting to be processed (`backlog`) and how many tickets the sale has
    /// left across its ticket types (`available`).
    ///
    /// A tick admits no more people than there are tickets left beyond the requests already
    /// waiting, each of which takes at least one: a sold-out sale admits nobody, and goes on
    /// with the next in line as tickets come back (an unpaid reservation expires or is
    /// cancelled, a ticket is refunded), so those tickets go to the front of the queue.
    pub fn next_admitted_through(
        &self,
        admit_per_tick: u32,
        max_backlog: u32,
        backlog: u64,
        available: u64,
    ) -> u64 {
        if backlog >= u64::from(max_backlog) {
            return self.admitted_through;
        }
        let left = available.saturating_sub(backlog);
        self.admitted_through
            .saturating_add(u64::from(admit_per_tick).min(left))
            .min(self.last_position)
    }

    /// Whether `position` has been admitted.
    pub const fn admits(&self, position: u64) -> bool {
        position <= self.admitted_through
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admits_a_batch_while_the_backlog_is_small() {
        let room = WaitingRoom {
            last_position: 1_000,
            admitted_through: 100,
        };
        assert_eq!(room.next_admitted_through(50, 200, 10, 10_000), 150);
    }

    #[test]
    fn pauses_while_the_backlog_is_large() {
        let room = WaitingRoom {
            last_position: 1_000,
            admitted_through: 100,
        };
        assert_eq!(room.next_admitted_through(50, 200, 200, 10_000), 100);
    }

    #[test]
    fn admits_no_more_people_than_tickets_left() {
        let room = WaitingRoom {
            last_position: 1_000,
            admitted_through: 100,
        };
        // Thirty tickets left, ten of them for requests already waiting.
        assert_eq!(room.next_admitted_through(50, 200, 10, 30), 120);
        assert_eq!(
            room.next_admitted_through(50, 200, 0, 0),
            100,
            "sold out: nobody"
        );
        assert_eq!(
            room.next_admitted_through(50, 200, 30, 30),
            100,
            "the waiting requests take what is left"
        );
    }

    #[test]
    fn never_admits_positions_not_yet_handed_out() {
        let room = WaitingRoom {
            last_position: 120,
            admitted_through: 100,
        };
        assert_eq!(room.next_admitted_through(50, 200, 0, 10_000), 120);
        assert!(room.admits(100));
        assert!(!room.admits(101));
    }
}
