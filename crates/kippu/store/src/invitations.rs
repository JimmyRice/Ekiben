use async_trait::async_trait;
use kippu_domain::invitation::Invitation;
use kippu_domain::{AccountId, SaleId};

use crate::{Insertion, Keyset, PageRequest, StoreResult};

/// The invitation lists of sales for invited accounts only.
#[async_trait]
pub trait InvitationStore {
    /// Puts an account on a sale's list.
    ///
    /// **Contract:** an account is on a sale's list at most once. Inviting it again writes
    /// nothing and returns the stored invitation, with its original time.
    async fn insert_invitation(
        &self,
        invitation: &Invitation,
    ) -> StoreResult<Insertion<Invitation>>;

    /// Takes an account off a sale's list. Returns whether it was on it.
    async fn delete_invitation(&self, sale: SaleId, account: AccountId) -> StoreResult<bool>;

    /// Whether an account is on a sale's list.
    async fn is_invited(&self, sale: SaleId, account: AccountId) -> StoreResult<bool>;

    /// A sale's list, most recently invited first (accounts invited together in account id
    /// order), resuming after the position `page.after` (`created_at` and account id).
    async fn invitations(
        &self,
        sale: SaleId,
        page: PageRequest<Keyset>,
    ) -> StoreResult<Vec<Invitation>>;
}
