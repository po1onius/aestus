//! GPT 账号代理设置，仅租户 owner 可写。
use super::gpt::GptAccountResponse;
use crate::{
    api::console::{auth::AdminUser, error::ConsoleResult},
    err::{AppError, ConsoleError},
    infra::account_proxy::ProxyUpdate,
    provider::{gpt::maintenance::GptMaintenance, service::ProviderResourceService},
    state::AppState,
};
use axum::{
    Json,
    extract::{Path, State},
};
use tracing::info;
use uuid::Uuid;

pub(super) async fn update(
    State(state): State<AppState>,
    AdminUser(owner): AdminUser,
    Path(id): Path<Uuid>,
    Json(input): Json<ProxyUpdate>,
) -> ConsoleResult<Json<GptAccountResponse>> {
    let tenant_id = owner
        .tenant_id
        .clone()
        .ok_or(AppError::Console(ConsoleError::Forbidden))?;
    let snapshot = ProviderResourceService::<GptMaintenance>::new(&state)
        .update_account_proxy(tenant_id, id, input)
        .await?;
    info!(account_id = %id, actor_id = %owner.id, "GPT 账号代理设置已保存并同步 runtime");
    Ok(Json(GptAccountResponse::from_snapshot(
        snapshot, true, true,
    )?))
}
