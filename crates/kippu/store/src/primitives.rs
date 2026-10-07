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

/// Keyset pagination: up to `limit` records in the listing's order, starting after the
/// record at `after`.
///
/// `P` is how a listing names a position: an id for listings in id order, a [`Keyset`] for
/// listings ordered by a time, a sequence number for logs. Positions only say where to resume;
/// which records a caller may see is decided by the listing's filter, never by a position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageRequest<P = uuid::Uuid> {
    /// Maximum number of records.
    pub limit: u32,
    /// Return records that sort after this position.
    pub after: Option<P>,
}

impl<P> PageRequest<P> {
    /// The first page of the given size.
    pub const fn first(limit: u32) -> Self {
        Self { limit, after: None }
    }

    /// The same request for one more record: whether that record comes back tells whether
    /// another page follows (see [`Page::from_lookahead`]).
    #[must_use]
    pub fn plus_one(self) -> Self {
        Self {
            limit: self.limit.saturating_add(1),
            after: self.after,
        }
    }
}

impl<P> Default for PageRequest<P> {
    fn default() -> Self {
        Self::first(50)
    }
}

/// A position in a listing ordered by a time, ties broken by id: the time and id of the last
/// record a page held.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Keyset {
    /// The time the listing sorts by, e.g. when an event starts.
    pub at: Timestamp,
    /// The record's id.
    pub id: uuid::Uuid,
}

/// One page of a listing, and where the next page starts.
#[derive(Debug, Clone, PartialEq)]
pub struct Page<T, P> {
    /// The records, in the listing's order.
    pub items: Vec<T>,
    /// The position to resume after; `None` on the last page.
    pub next: Option<P>,
}

impl<T, P> Page<T, P> {
    /// A listing that always fits one page.
    pub const fn all(items: Vec<T>) -> Self {
        Self { items, next: None }
    }

    /// Turns the records of a [`PageRequest::plus_one`] read into a page of `limit`: a record
    /// beyond `limit` means another page follows, resuming after the last record kept.
    pub fn from_lookahead(mut records: Vec<T>, limit: u32, position: impl FnOnce(&T) -> P) -> Self {
        let limit = usize::try_from(limit).unwrap_or(usize::MAX);
        let next = if records.len() > limit {
            records.truncate(limit);
            records.last().map(position)
        } else {
            None
        };
        Self {
            items: records,
            next,
        }
    }

    /// Converts each record, keeping the position of the next page.
    pub fn map<U>(self, convert: impl FnMut(T) -> U) -> Page<U, P> {
        Page {
            items: self.items.into_iter().map(convert).collect(),
            next: self.next,
        }
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
