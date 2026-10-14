//! Waiting-room use cases, independent of HTTP: joining, asking whether it is your turn,
//! and the task that lets the next batch in.

use kippu_domain::SaleId;
use kippu_domain::admission::{AdmissionPolicy, WaitingRoom};
use kippu_store::BoxError;

use crate::app::AppState;
use crate::auth::tokens::IssuedToken;
use crate::auth::{Principal, Scope};
use crate::error::{ApiError, ApiResult, ProblemKind, StatusCode};
use crate::module::Progress;
use crate::modules::catalog::service::{visible_sale, writable_sale};
use crate::modules::purchasing::permissions::PURCHASES_CREATE;

/// A buyer's place in a waiting room.
#[derive(Debug, Clone)]
pub struct QueueTicket {
    /// The position (1 is first).
    pub position: u64,
    /// Proves the position; presented when asking for admission.
    pub token: IssuedToken,
}

/// Whether a buyer may buy yet.
#[derive(Debug, Clone)]
pub enum Admission {
    /// Their turn: the pass to send with purchase requests.
    Admitted(IssuedToken),
    /// Not yet.
    Waiting {
        /// Their position.
        position: u64,
        /// Everyone up to this position has been admitted.
        admitted_through: u64,
    },
}

fn admission_required() -> ApiError {
    ApiError::new(
        StatusCode::FORBIDDEN,
        ProblemKind::ADMISSION_REQUIRED,
        "join the waiting room first",
    )
}

#[track_caller]
fn no_waiting_room() -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        ProblemKind::NO_WAITING_ROOM,
        "this sale has no waiting room",
    )
}

/// How far a sale's waiting room has got, for its organizers: the last position handed out
/// and how far admission has reached.
#[tracing::instrument(skip_all)]
pub async fn waiting_room(
    state: &AppState,
    principal: &Principal,
    sale_id: SaleId,
) -> ApiResult<WaitingRoom> {
    let (sale, _) = writable_sale(state, principal, sale_id).await?;
    if !matches!(sale.admission, AdmissionPolicy::WaitingRoom { .. }) {
        return Err(no_waiting_room());
    }
    Ok(state.store().waiting_room(sale.id).await?)
}

/// Joins a sale's waiting room.
#[tracing::instrument(skip_all)]
pub async fn join_waiting_room(
    state: &AppState,
    principal: &Principal,
    sale_id: SaleId,
) -> ApiResult<QueueTicket> {
    let account = principal.require_account()?;
    state.authorize(principal, PURCHASES_CREATE, Scope::Account(account))?;
    let (sale, _) = visible_sale(state, Some(principal), sale_id).await?;
    if !matches!(sale.admission, AdmissionPolicy::WaitingRoom { .. }) {
        return Err(no_waiting_room());
    }
    let position = state.store().join_waiting_room(sale.id).await?;
    let token = state
        .tokens()
        .issue_queue_ticket(account, sale.id, position, state.now());
    Ok(QueueTicket { position, token })
}

/// Asks whether it is the caller's turn to buy, presenting the queue ticket from joining.
/// Sales without a waiting room admit everyone.
#[tracing::instrument(skip_all)]
pub async fn request_admission(
    state: &AppState,
    principal: &Principal,
    sale_id: SaleId,
    queue_ticket: Option<String>,
) -> ApiResult<Admission> {
    let account = principal.require_account()?;
    state.authorize(principal, PURCHASES_CREATE, Scope::Account(account))?;
    let (sale, _) = visible_sale(state, Some(principal), sale_id).await?;
    let now = state.now();
    if let AdmissionPolicy::WaitingRoom { .. } = sale.admission {
        let ticket = queue_ticket.ok_or_else(admission_required)?;
        let position = state
            .tokens()
            .verify_queue_ticket(&ticket, account, sale.id, now)?;
        let room = state.store().waiting_room(sale.id).await?;
        if !room.admits(position) {
            return Ok(Admission::Waiting {
                position,
                admitted_through: room.admitted_through,
            });
        }
    }
    Ok(Admission::Admitted(
        state.tokens().issue_admission_pass(account, sale.id, now),
    ))
}

/// Background task: advances every waiting room by one batch, unless its sale has not opened
/// yet or its purchase backlog is already large. Safe on many instances: the advance is a
/// compare-and-set.
#[tracing::instrument(skip_all)]
pub(crate) async fn admit_batches(state: AppState) -> Result<Progress, BoxError> {
    let store = state.store();
    let now = state.now();
    for sale_id in store.pending_waiting_rooms().await? {
        let Some(sale) = store.sale(sale_id).await? else {
            continue;
        };
        let AdmissionPolicy::WaitingRoom {
            admit_per_tick,
            max_backlog,
        } = sale.admission
        else {
            continue;
        };
        // Before the sale opens nobody can buy, so the backlog stays empty and the whole room
        // would be let through before the first purchase. A closed sale keeps draining: its
        // buyers are turned away by the purchase itself, and the room stops being pending.
        if now < sale.opens_at {
            continue;
        }
        let room = store.waiting_room(sale_id).await?;
        // Only whether the backlog reached the limit matters, so counting stops there.
        let backlog = store.queued_purchase_count(sale_id, max_backlog).await?;
        let next = room.next_admitted_through(admit_per_tick, max_backlog, backlog);
        if next > room.admitted_through
            && store
                .advance_waiting_room(sale_id, room.admitted_through, next)
                .await?
        {
            tracing::info!(sale = %sale_id, admitted_through = next, backlog, "admitted a batch");
        }
    }
    Ok(Progress::Idle)
}
