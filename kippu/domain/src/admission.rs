//! Admission control: who may submit a purchase request, and when.
//!
//! A waiting room hands out queue positions in arrival order and admits them in batches.
//! A batch is released only while the purchase backlog is small, so admissions follow the
//! speed at which workers actually process purchases — the queue applies back-pressure all
//! the way to the buyer's browser.

use serde::{Deserialize, Serialize};

use crate::validation::ValidationError;

/// How buyers are let into a sale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum AdmissionPolicy {
    /// Anyone may submit purchase requests while the sale is open.
    Open,
    /// Buyers join a queue and are admitted in batches.
    WaitingRoom {
        /// How many queue positions are admitted per admission tick.
        admit_per_tick: u32,
        /// Admission pauses while this many purchase requests are waiting to be processed.
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
pub struct WaitingRoom {
    /// Highest queue position handed out so far (positions start at 1).
    pub last_position: u64,
    /// Every position up to and including this one has been admitted.
    pub admitted_through: u64,
}

impl WaitingRoom {
    /// The `admitted_through` mark after one admission tick, given how many purchase
    /// requests are still waiting to be processed.
    pub fn next_admitted_through(
        &self,
        admit_per_tick: u32,
        max_backlog: u32,
        backlog: u64,
    ) -> u64 {
        if backlog >= u64::from(max_backlog) {
            return self.admitted_through;
        }
        self.admitted_through
            .saturating_add(u64::from(admit_per_tick))
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
        assert_eq!(room.next_admitted_through(50, 200, 10), 150);
    }

    #[test]
    fn pauses_while_the_backlog_is_large() {
        let room = WaitingRoom {
            last_position: 1_000,
            admitted_through: 100,
        };
        assert_eq!(room.next_admitted_through(50, 200, 200), 100);
    }

    #[test]
    fn never_admits_positions_not_yet_handed_out() {
        let room = WaitingRoom {
            last_position: 120,
            admitted_through: 100,
        };
        assert_eq!(room.next_admitted_through(50, 200, 0), 120);
        assert!(room.admits(100));
        assert!(!room.admits(101));
    }
}
