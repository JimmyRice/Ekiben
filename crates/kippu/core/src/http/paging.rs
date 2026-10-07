//! Pagination over HTTP: the `limit` and `cursor` parameters of paged listings, the opaque
//! cursors themselves, and [`Listing`], the `{items, next_cursor}` body every listing returns.
//!
//! A cursor is the base64url form of one byte naming the listing ([`ListingTag`]) followed by
//! the position of the last item the previous page held (an id, a time and an id, or a
//! sequence number). Listings read in index order from that position (keyset pagination), so
//! a page costs the same however deep it is and items added meanwhile neither shift nor
//! repeat a page. A cursor only says where to resume: what the caller may see is decided by
//! the listing's filter, so a forged cursor reveals nothing.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use kippu_domain::Timestamp;
use kippu_store::{Keyset, Page, PageRequest};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::error::{ApiError, StatusCode};

/// Items per page when the client does not say.
const DEFAULT_LIMIT: u32 = 50;
/// The most items a page may hold.
const MAX_LIMIT: u32 = 200;

/// Names a listing inside its cursors, so a cursor only resumes the listing (and order) it
/// came from. Kippu's own listings use values below 128; a fork's modules pick theirs from
/// 128 up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListingTag(pub u8);

impl ListingTag {
    pub(crate) const ACCOUNTS: Self = Self(1);
    pub(crate) const ORGANIZATIONS: Self = Self(2);
    pub(crate) const AUDIT_LOG: Self = Self(3);
    pub(crate) const TICKETS: Self = Self(4);
    pub(crate) const RESERVATIONS: Self = Self(5);
    pub(crate) const FAVORITES: Self = Self(6);
    pub(crate) const EVENTS_BY_START: Self = Self(16);
    pub(crate) const EVENTS_BY_START_DESC: Self = Self(17);
    pub(crate) const EVENTS_BY_CREATION: Self = Self(18);
    pub(crate) const EVENTS_BY_CREATION_DESC: Self = Self(19);
}

/// A position a cursor can carry: the key of the last item of a page.
pub trait CursorPosition: Sized {
    /// Appends the position's bytes.
    fn write(&self, bytes: &mut Vec<u8>);
    /// Reads a position written by [`write`](Self::write); `None` if `bytes` are not one.
    fn read(bytes: &[u8]) -> Option<Self>;
}

impl CursorPosition for uuid::Uuid {
    fn write(&self, bytes: &mut Vec<u8>) {
        bytes.extend_from_slice(self.as_bytes());
    }

    fn read(bytes: &[u8]) -> Option<Self> {
        Self::from_slice(bytes).ok()
    }
}

impl CursorPosition for i64 {
    fn write(&self, bytes: &mut Vec<u8>) {
        bytes.extend_from_slice(&self.to_be_bytes());
    }

    fn read(bytes: &[u8]) -> Option<Self> {
        Some(Self::from_be_bytes(bytes.try_into().ok()?))
    }
}

impl CursorPosition for Keyset {
    fn write(&self, bytes: &mut Vec<u8>) {
        self.at.unix_micros().write(bytes);
        self.id.write(bytes);
    }

    fn read(bytes: &[u8]) -> Option<Self> {
        let (at, id) = bytes.split_at_checked(8)?;
        Some(Self {
            at: Timestamp::from_unix_micros(i64::read(at)?),
            id: uuid::Uuid::read(id)?,
        })
    }
}

/// Encodes a position as a cursor of `listing`.
pub fn encode_cursor(listing: ListingTag, position: &impl CursorPosition) -> String {
    let mut bytes = vec![listing.0];
    position.write(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Decodes a cursor into the listing it names and its position.
///
/// # Errors
///
/// `400 invalid-parameter` if `cursor` is not a cursor holding a `P`.
pub fn decode_cursor<P: CursorPosition>(cursor: &str) -> Result<(ListingTag, P), ApiError> {
    URL_SAFE_NO_PAD
        .decode(cursor)
        .ok()
        .and_then(|bytes| {
            let (&tag, position) = bytes.split_first()?;
            Some((ListingTag(tag), P::read(position)?))
        })
        .ok_or_else(|| invalid_cursor("cursor is not a cursor Kippu issued"))
}

/// The error for a cursor that cannot resume the listing it was sent to.
#[track_caller]
pub fn invalid_cursor(detail: &'static str) -> ApiError {
    ApiError::new(StatusCode::BAD_REQUEST, "invalid-parameter", detail)
}

/// Page parameters shared by paged listings.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in = Query)]
pub struct PageQuery {
    /// At most this many items (default 50, at most 200). A page may hold fewer even when
    /// more follow; only a `null` `next_cursor` means the end.
    pub limit: Option<u32>,
    /// Where to continue: the `next_cursor` of the previous page. Leave out for the first
    /// page, and keep the other parameters as they were.
    pub cursor: Option<String>,
}

/// The page size to use for a `limit` parameter: 50 when absent, at most 200.
pub fn page_limit(limit: Option<u32>) -> u32 {
    limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
}

impl PageQuery {
    /// The page size asked for, within bounds.
    pub fn limit(&self) -> u32 {
        page_limit(self.limit)
    }

    /// The page to read from the store, for a listing named `listing`.
    ///
    /// # Errors
    ///
    /// `400 invalid-parameter` if `cursor` is not a cursor of this listing.
    pub fn page<P: CursorPosition>(&self, listing: ListingTag) -> Result<PageRequest<P>, ApiError> {
        let after = match self.cursor.as_deref() {
            None => None,
            Some(cursor) => match decode_cursor(cursor)? {
                (tag, position) if tag == listing => Some(position),
                _ => return Err(invalid_cursor("cursor belongs to another listing")),
            },
        };
        Ok(PageRequest {
            limit: self.limit(),
            after,
        })
    }
}

/// A listing: one page of items, and the cursor of the next page.
#[derive(Debug, Serialize, ToSchema)]
pub struct Listing<T> {
    /// The items, in the listing's order.
    pub items: Vec<T>,
    /// Pass as `cursor` to read the next page; `null` on the last page. Opaque: do not
    /// build or take it apart.
    pub next_cursor: Option<String>,
}

impl<T> Listing<T> {
    /// A listing that always fits one page.
    pub const fn all(items: Vec<T>) -> Self {
        Self {
            items,
            next_cursor: None,
        }
    }

    /// One page of a listing named `listing`, its items converted for the response.
    pub fn page<S, P: CursorPosition>(
        page: Page<S, P>,
        listing: ListingTag,
        convert: impl FnMut(S) -> T,
    ) -> Self {
        Self {
            items: page.items.into_iter().map(convert).collect(),
            next_cursor: page.next.map(|position| encode_cursor(listing, &position)),
        }
    }
}

impl<T> From<Vec<T>> for Listing<T> {
    fn from(items: Vec<T>) -> Self {
        Self::all(items)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursors_round_trip_and_name_their_listing() {
        let position = Keyset {
            at: Timestamp::from_unix_micros(1_800_000_000_123_456),
            id: uuid::Uuid::now_v7(),
        };
        let cursor = encode_cursor(ListingTag::TICKETS, &position);
        assert!(
            cursor
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        );
        assert_eq!(
            decode_cursor::<Keyset>(&cursor).unwrap(),
            (ListingTag::TICKETS, position)
        );

        let query = PageQuery {
            limit: Some(1_000),
            cursor: Some(cursor),
        };
        let page = query.page::<Keyset>(ListingTag::TICKETS).unwrap();
        assert_eq!((page.limit, page.after), (MAX_LIMIT, Some(position)));
        assert!(query.page::<Keyset>(ListingTag::RESERVATIONS).is_err());
    }

    #[test]
    fn malformed_cursors_are_rejected() {
        for cursor in ["", "not base64!", "AQ", "AQID"] {
            assert!(decode_cursor::<Keyset>(cursor).is_err(), "{cursor:?}");
        }
        let short = encode_cursor(ListingTag::AUDIT_LOG, &7_i64);
        assert!(decode_cursor::<uuid::Uuid>(&short).is_err());
        assert_eq!(
            decode_cursor::<i64>(&short).unwrap(),
            (ListingTag::AUDIT_LOG, 7)
        );
    }

    #[test]
    fn listings_carry_the_next_cursor_only_when_more_follow() {
        let last = Page::<u8, i64>::all(vec![1, 2]);
        let listing = Listing::page(last, ListingTag::AUDIT_LOG, u32::from);
        assert_eq!((listing.items, listing.next_cursor), (vec![1, 2], None));

        let more = Page::from_lookahead(vec![5_i64, 4, 3], 2, |&sequence| sequence);
        let listing = Listing::page(more, ListingTag::AUDIT_LOG, |sequence| sequence);
        assert_eq!(listing.items, vec![5, 4]);
        let next = decode_cursor::<i64>(&listing.next_cursor.unwrap()).unwrap();
        assert_eq!(next, (ListingTag::AUDIT_LOG, 4));
    }
}
