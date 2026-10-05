use crate::http::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use kippu_domain::SaleId;
use kippu_domain::admission::AdmissionPolicy;

use super::dto::{AdmissionRequest, AdmissionView, QueuePlace};
use crate::app::AppState;
use crate::auth::{Principal, Scope};
use crate::error::{ApiError, ApiResult, Problem};
use crate::modules::catalog::service::visible_sale;
use crate::modules::purchasing::permissions::PURCHASES_CREATE;

const TAG: &str = "admission";

/// Join a sale's waiting room.
#[utoipa::path(
    post, path = "/v1/sales/{sale_id}/waiting-room", tag = TAG,
    security(("bearer" = [])),
    params(("sale_id" = SaleId, Path)),
    responses((status = 200, body = QueuePlace), (status = 409, description = "The sale has no waiting room", body = Problem))
)]
pub(crate) async fn join_waiting_room(
    State(state): State<AppState>,
    principal: Principal,
    Path(sale_id): Path<SaleId>,
) -> ApiResult<Json<QueuePlace>> {
    let account = principal.require_account()?;
    state.authorize(&principal, PURCHASES_CREATE, Scope::Account(account))?;
    let (sale, _) = visible_sale(&state, Some(&principal), sale_id).await?;
    if !matches!(sale.admission, AdmissionPolicy::WaitingRoom { .. }) {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "no-waiting-room",
            "this sale has no waiting room",
        ));
    }
    let position = state.store().join_waiting_room(sale.id).await?;
    let queue_ticket = state
        .tokens()
        .issue_queue_ticket(account, sale.id, position, state.now());
    Ok(Json(QueuePlace {
        position,
        queue_ticket,
    }))
}

/// Ask whether it is your turn to buy. Sales without a waiting room admit everyone.
#[utoipa::path(
    post, path = "/v1/sales/{sale_id}/admission", tag = TAG,
    security(("bearer" = [])),
    params(("sale_id" = SaleId, Path)),
    request_body = AdmissionRequest,
    responses((status = 200, body = AdmissionView), (status = 403, body = Problem))
)]
pub(crate) async fn request_admission(
    State(state): State<AppState>,
    principal: Principal,
    Path(sale_id): Path<SaleId>,
    Json(request): Json<AdmissionRequest>,
) -> ApiResult<Json<AdmissionView>> {
    let account = principal.require_account()?;
    state.authorize(&principal, PURCHASES_CREATE, Scope::Account(account))?;
    let (sale, _) = visible_sale(&state, Some(&principal), sale_id).await?;
    let now = state.now();
    if let AdmissionPolicy::WaitingRoom { .. } = sale.admission {
        let ticket = request.queue_ticket.ok_or_else(|| {
            ApiError::new(
                StatusCode::FORBIDDEN,
                "admission-required",
                "join the waiting room first",
            )
        })?;
        let position = state
            .tokens()
            .verify_queue_ticket(&ticket, account, sale.id, now)?;
        let room = state.store().waiting_room(sale.id).await?;
        if !room.admits(position) {
            return Ok(Json(AdmissionView::Waiting {
                position,
                admitted_through: room.admitted_through,
            }));
        }
    }
    let admission_pass = state.tokens().issue_admission_pass(account, sale.id, now);
    Ok(Json(AdmissionView::Admitted { admission_pass }))
}
