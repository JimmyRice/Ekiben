//! Request and response bodies of the denial endpoints.

use kippu_domain::denial::DenialSubject;
use serde::Deserialize;
use utoipa::ToSchema;

use super::service::{DenialChanges, NewDenial};

/// A ticket or an account to refuse entry.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateDenialRequest {
    /// What to refuse: `{"kind": "ticket", "id": …}` or `{"kind": "account", "id": …}`.
    pub subject: DenialSubject,
    /// Why, for staff; at most 1000 characters.
    #[serde(default)]
    pub note: String,
}

/// Changes to a denial: only the fields to change, plus the `version` you last read.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DenialPatch {
    /// The version you are editing; a mismatch fails with 412.
    pub version: i64,
    /// Why, for staff; at most 1000 characters.
    pub note: Option<String>,
}

impl From<CreateDenialRequest> for NewDenial {
    fn from(request: CreateDenialRequest) -> Self {
        Self {
            subject: request.subject,
            note: request.note,
        }
    }
}

impl From<DenialPatch> for DenialChanges {
    fn from(patch: DenialPatch) -> Self {
        Self { note: patch.note }
    }
}
