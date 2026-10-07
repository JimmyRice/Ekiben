use async_trait::async_trait;
use kippu_domain::denial::{Denial, DeniedTicket};
use kippu_domain::{AccountId, DenialId, EventId, OrganizationId};

use crate::{Insertion, Keyset, PageRequest, StoreResult};

/// Deny lists, and the tickets they (and refunds) make gates refuse.
#[async_trait]
pub trait DenialStore {
    /// Adds a denial.
    ///
    /// **Contract:** the id is unique. Adding a denial whose id exists writes nothing and
    /// returns the stored one, so a derived id makes adding idempotent.
    async fn insert_denial(&self, denial: &Denial) -> StoreResult<Insertion<Denial>>;

    /// Looks a denial up by id.
    async fn denial(&self, id: DenialId) -> StoreResult<Option<Denial>>;

    /// Replaces a denial's note, update time and version if its stored version is still
    /// `expected_version`; fails with `Conflict("version")` otherwise.
    async fn update_denial(&self, denial: &Denial, expected_version: i64) -> StoreResult<()>;

    /// Lifts a denial. Returns whether it existed.
    async fn delete_denial(&self, id: DenialId) -> StoreResult<bool>;

    /// An organization's denials for all of its events (not those for one event), most recently
    /// added first (denials added together in id order), resuming after `page.after`.
    async fn organization_denials(
        &self,
        organization: OrganizationId,
        page: PageRequest<Keyset>,
    ) -> StoreResult<Vec<Denial>>;

    /// An event's own denials (not its organization's), most recently added first (denials
    /// added together in id order), resuming after `page.after`.
    async fn event_denials(
        &self,
        event: EventId,
        page: PageRequest<Keyset>,
    ) -> StoreResult<Vec<Denial>>;

    /// Whether `account` is denied entry to `event` of `organization`: by a denial for the
    /// event, or for every event of the organization.
    async fn account_denied(
        &self,
        organization: OrganizationId,
        event: EventId,
        account: AccountId,
    ) -> StoreResult<bool>;

    /// The tickets of `event` a gate must refuse, in ticket id order, resuming after
    /// `page.after`: tickets revoked, tickets denied, and tickets of denied accounts — denied
    /// for the event, or for every event of its organization. A revoked ticket is reported as
    /// [`Revoked`](kippu_domain::denial::DeniedBecause::Revoked) even when it is also denied.
    async fn denied_tickets(
        &self,
        event: EventId,
        page: PageRequest<uuid::Uuid>,
    ) -> StoreResult<Vec<DeniedTicket>>;
}
