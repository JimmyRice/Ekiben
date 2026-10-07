use async_trait::async_trait;
use kippu_domain::Timestamp;

use crate::{Insertion, PageRequest, StoreResult};

/// The response to a request made with an `Idempotency-Key`, kept so a retry gets the same
/// answer without running the request again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdempotencyRecord {
    /// Digest of the original request, to detect a key reused for a different request.
    pub fingerprint: String,
    /// HTTP status of the original response.
    pub status: u16,
    /// Body of the original response.
    pub body: Vec<u8>,
    /// When the original request was handled.
    pub created_at: Timestamp,
}

/// One privileged action, for the append-only audit log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEntry {
    /// When it happened.
    pub at: Timestamp,
    /// Who did it, e.g. `root:alice-yubikey` or an account id.
    pub actor: String,
    /// What they did, e.g. `account.create`.
    pub action: String,
    /// What they did it to.
    pub target: String,
}

/// An audit log entry as stored, with its place in the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditRecord {
    /// Position in the log; later entries have larger numbers.
    pub sequence: i64,
    /// The entry.
    pub entry: AuditEntry,
}

/// Idempotency records and the audit log.
#[async_trait]
pub trait HousekeepingStore {
    /// The stored response for `key` within `scope` (typically the caller's identity).
    async fn idempotency_record(
        &self,
        scope: &str,
        key: &str,
    ) -> StoreResult<Option<IdempotencyRecord>>;

    /// Stores a response. If one already exists for `(scope, key)`, keeps it and returns it.
    async fn save_idempotency_record(
        &self,
        scope: &str,
        key: &str,
        record: &IdempotencyRecord,
    ) -> StoreResult<Insertion<IdempotencyRecord>>;

    /// Appends to the audit log.
    async fn append_audit(&self, entry: &AuditEntry) -> StoreResult<()>;

    /// Reads audit entries newest first, resuming before the sequence number `page.after`.
    async fn audit_log(&self, page: PageRequest<i64>) -> StoreResult<Vec<AuditRecord>>;
}
