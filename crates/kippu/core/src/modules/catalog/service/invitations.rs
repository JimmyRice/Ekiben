//! Invitation lists: the accounts a sale for invited accounts only is open to.

use kippu_domain::catalog::{Audience, Sale};
use kippu_domain::invitation::Invitation;
use kippu_domain::{AccountId, SaleId};
use kippu_store::{Insertion, Keyset, Page, PageRequest};

use super::writable_sale;
use crate::app::AppState;
use crate::auth::Principal;
use crate::error::{ApiError, ApiResult, ProblemKind, StatusCode};

/// An invitation that was asked for, and whether it is new: inviting an account again returns
/// the invitation already there, unchanged.
#[derive(Debug, Clone, Copy)]
pub struct AddedInvitation {
    /// The invitation.
    pub invitation: Invitation,
    /// Whether this call added it.
    pub created: bool,
}

/// A sale whose invitation list the caller may keep: one it may edit, and not private (nobody
/// buys from those, so a list would mean nothing).
async fn listed_sale(state: &AppState, principal: &Principal, id: SaleId) -> ApiResult<Sale> {
    let (sale, _) = writable_sale(state, principal, id).await?;
    if sale.audience == Audience::Private {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            ProblemKind::WRONG_SALE_AUDIENCE,
            "a private sale has no invitation list: grant its tickets instead",
        ));
    }
    Ok(sale)
}

/// Puts an account on a sale's invitation list. The list may be kept while the sale is still
/// public; it matters once the sale is for invited accounts only. Inviting an account again
/// changes nothing.
#[tracing::instrument(skip_all)]
pub async fn invite(
    state: &AppState,
    principal: &Principal,
    sale_id: SaleId,
    account_id: AccountId,
) -> ApiResult<AddedInvitation> {
    let sale = listed_sale(state, principal, sale_id).await?;
    let account = state
        .store()
        .account(account_id)
        .await?
        .ok_or_else(|| ApiError::not_found("account"))?;
    if account.is_closed() {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            ProblemKind::ACCOUNT_CLOSED,
            "this account is closed",
        ));
    }
    let invitation = Invitation {
        sale_id: sale.id,
        account_id: account.id,
        created_at: state.now(),
    };
    match state.store().insert_invitation(&invitation).await? {
        Insertion::Inserted => {
            state
                .audit(
                    principal,
                    "sale.invite",
                    format!("{}/{}", sale.id, account.id),
                )
                .await?;
            tracing::info!(sale = %sale.id, account = %account.id, "account invited");
            Ok(AddedInvitation {
                invitation,
                created: true,
            })
        }
        Insertion::Existing(existing) => Ok(AddedInvitation {
            invitation: existing,
            created: false,
        }),
    }
}

/// Takes an account off a sale's invitation list: it buys no more from the sale once the sale
/// is for invited accounts only. Tickets it holds stay valid.
#[tracing::instrument(skip_all)]
pub async fn uninvite(
    state: &AppState,
    principal: &Principal,
    sale_id: SaleId,
    account_id: AccountId,
) -> ApiResult<()> {
    let sale = listed_sale(state, principal, sale_id).await?;
    if !state.store().delete_invitation(sale.id, account_id).await? {
        return Err(ApiError::not_found("invitation"));
    }
    state
        .audit(
            principal,
            "sale.uninvite",
            format!("{}/{account_id}", sale.id),
        )
        .await?;
    tracing::info!(sale = %sale.id, account = %account_id, "invitation withdrawn");
    Ok(())
}

/// A sale's invitation list, most recently invited first.
#[tracing::instrument(skip_all)]
pub async fn invitations(
    state: &AppState,
    principal: &Principal,
    sale_id: SaleId,
    page: PageRequest<Keyset>,
) -> ApiResult<Page<Invitation, Keyset>> {
    let sale = listed_sale(state, principal, sale_id).await?;
    let invitations = state.store().invitations(sale.id, page.plus_one()).await?;
    Ok(Page::from_lookahead(
        invitations,
        page.limit,
        |invitation| Keyset {
            at: invitation.created_at,
            id: invitation.account_id.as_uuid(),
        },
    ))
}
