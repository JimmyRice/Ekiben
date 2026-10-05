use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::auth::tokens::IssuedToken;

/// Your place in the waiting room.
#[derive(Debug, Serialize, ToSchema)]
pub struct QueuePlace {
    /// Your position (1 is first).
    pub position: u64,
    /// Present this to `/admission` to check whether it is your turn.
    pub queue_ticket: IssuedToken,
}

/// Ask whether it is your turn.
#[derive(Debug, Default, Deserialize, ToSchema)]
pub struct AdmissionRequest {
    /// The ticket from joining the waiting room. Not needed for sales without one.
    pub queue_ticket: Option<String>,
}

/// Whether you may buy yet.
#[derive(Debug, Serialize, ToSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum AdmissionView {
    /// Your turn: send this pass as `Kippu-Admission-Pass` when submitting purchase requests.
    Admitted {
        /// The pass.
        admission_pass: IssuedToken,
    },
    /// Not yet. Ask again in a few seconds.
    Waiting {
        /// Your position.
        position: u64,
        /// Everyone up to this position has been admitted.
        admitted_through: u64,
    },
}
