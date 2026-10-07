//! Small types several ports share: insertion outcomes, pages and leases.

use kippu_domain::Timestamp;

/// The outcome of inserting a record under a unique key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Insertion<T> {
    /// The record is new.
    Inserted,
    /// A record with the same key already exists; here it is.
    Existing(T),
}

/// Keyset pagination: up to `limit` records ordered by id, starting after `after`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageRequest {
    /// Maximum number of records.
    pub limit: u32,
    /// Return records whose id sorts after this one.
    pub after: Option<uuid::Uuid>,
}

impl PageRequest {
    /// The first page of the given size.
    pub const fn first(limit: u32) -> Self {
        Self { limit, after: None }
    }
}

impl Default for PageRequest {
    fn default() -> Self {
        Self::first(50)
    }
}

/// A lease on queued work, held until `until` by whichever worker claimed it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lease {
    /// When the claim is taken.
    pub now: Timestamp,
    /// When the claim lapses if the worker dies without finishing.
    pub until: Timestamp,
}
