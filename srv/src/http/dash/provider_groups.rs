use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{get, post, put},
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    err::{AdminResult, AppError, AppResult},
    http::dash::auth,
    provider::{
        claude::maintenance::ClaudeMaintenance,
        gpt::{maintenance::GptMaintenance, model as gpt_model},
        group::{self, ProviderGroupSummary, ProviderGroupWithModels, UnassignedProviderResource},
        service::ProviderResourceService,
    },
    state::AppState,
};

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct GroupListQuery {
    provider: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateGroupRequest {
    provider: String,
    name: String,
    models: Vec<String>,
    account_ids: Vec<Uuid>,
    api_key_ids: Vec<Uuid>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RenameGroupRequest {
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateGroupEnabledRequest {
    enabled: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateGroupModelsRequest {
    models: Vec<String>,
}

#[derive(Debug, Serialize)]
struct DeleteGroupResponse {
    id: Uuid,
    provider: String,
    name: String,
    released_account_count: usize,
    released_upstream_api_key_count: usize,
    deleted_gateway_api_key_count: usize,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/options", get(list_group_options))
        .route("/unassigned-resources", get(list_unassigned_resources))
        .route("/", get(list_groups).post(create_group))
        .route("/{id}", put(rename_group).delete(delete_group))
        .route("/{id}/models", put(update_group_models))
        .route("/{id}/enabled", post(update_group_enabled))
}

async fn list_unassigned_resources(
    State(state): State<AppState>,
    auth::AdminUser(owner): auth::AdminUser,
    Query(query): Query<GroupListQuery>,
) -> AdminResult<Json<Vec<UnassignedProviderResource>>> {
    let provider = query
        .provider
        .as_deref()
        .map(str::trim)
        .filter(|provider| !provider.is_empty())
        .ok_or_else(|| crate::err::AppError::BadRequest {
            message: "provider 不能为空".to_owned(),
        })?;
    let mut conn = state.db_conn().await?;
    let tenant_id = owner.tenant_id.ok_or(AppError::Forbidden)?;
    Ok(Json(
        group::list_unassigned_resources(&mut conn, tenant_id, provider).await?,
    ))
}

/// 分组名称不是凭证信息。普通用户创建网关 Key 时需要读取启用分组，因此这里使用
/// CurrentUser；管理统计与所有写操作仍严格要求租户 owner。
async fn list_group_options(
    State(state): State<AppState>,
    auth::CurrentUser(current_user): auth::CurrentUser,
) -> AppResult<Json<Vec<ProviderGroupWithModels>>> {
    let tenant_id = current_user.tenant_id.ok_or(AppError::Forbidden)?;
    let mut conn = state.db_conn().await?;
    Ok(Json(group::list_enabled(&mut conn, tenant_id).await?))
}

async fn list_groups(
    State(state): State<AppState>,
    auth::AdminUser(owner): auth::AdminUser,
    Query(query): Query<GroupListQuery>,
) -> AdminResult<Json<Vec<ProviderGroupSummary>>> {
    let provider = query
        .provider
        .as_deref()
        .map(str::trim)
        .filter(|provider| !provider.is_empty());
    let mut conn = state.db_conn().await?;
    let tenant_id = owner.tenant_id.ok_or(AppError::Forbidden)?;
    Ok(Json(
        group::list_summaries(&mut conn, tenant_id, provider).await?,
    ))
}

async fn create_group(
    State(state): State<AppState>,
    auth::AdminUser(owner): auth::AdminUser,
    Json(payload): Json<CreateGroupRequest>,
) -> AdminResult<Json<ProviderGroupWithModels>> {
    let mut conn = state.db_conn().await?;
    let tenant_id = owner.tenant_id.ok_or(AppError::Forbidden)?;
    let created = group::create(
        &mut conn,
        tenant_id,
        payload.provider,
        payload.name,
        payload.models,
        payload.account_ids,
        payload.api_key_ids,
    )
    .await?;
    drop(conn);

    let provider = created.group.group.provider.as_str();
    match provider {
        gpt_model::PROVIDER => {
            ProviderResourceService::<GptMaintenance>::new(&state)
                .sync_resource_snapshots(created.accounts, created.api_keys)
                .await?;
        }
        crate::provider::claude::model::PROVIDER => {
            ProviderResourceService::<ClaudeMaintenance>::new(&state)
                .sync_resource_snapshots(created.accounts, created.api_keys)
                .await?;
        }
        _ => unreachable!("group::create 已验证 provider"),
    }

    Ok(Json(created.group))
}

async fn rename_group(
    State(state): State<AppState>,
    auth::AdminUser(owner): auth::AdminUser,
    Path(id): Path<Uuid>,
    Json(payload): Json<RenameGroupRequest>,
) -> AdminResult<Json<ProviderGroupWithModels>> {
    let mut conn = state.db_conn().await?;
    let tenant_id = owner.tenant_id.ok_or(AppError::Forbidden)?;
    let group = group::rename(&mut conn, tenant_id, id, payload.name).await?;
    Ok(Json(group::with_models(&mut conn, group).await?))
}

async fn update_group_enabled(
    State(state): State<AppState>,
    auth::AdminUser(owner): auth::AdminUser,
    Path(id): Path<Uuid>,
    Json(payload): Json<UpdateGroupEnabledRequest>,
) -> AdminResult<Json<ProviderGroupWithModels>> {
    let mut conn = state.db_conn().await?;
    let tenant_id = owner.tenant_id.ok_or(AppError::Forbidden)?;
    let group = group::update_enabled(&mut conn, tenant_id, id, payload.enabled).await?;
    Ok(Json(group::with_models(&mut conn, group).await?))
}

async fn update_group_models(
    State(state): State<AppState>,
    auth::AdminUser(owner): auth::AdminUser,
    Path(id): Path<Uuid>,
    Json(payload): Json<UpdateGroupModelsRequest>,
) -> AdminResult<Json<ProviderGroupWithModels>> {
    let mut conn = state.db_conn().await?;
    let tenant_id = owner.tenant_id.ok_or(AppError::Forbidden)?;
    Ok(Json(
        group::update_models(&mut conn, tenant_id, id, payload.models).await?,
    ))
}

async fn delete_group(
    State(state): State<AppState>,
    auth::AdminUser(owner): auth::AdminUser,
    Path(id): Path<Uuid>,
) -> AdminResult<Json<DeleteGroupResponse>> {
    let mut conn = state.db_conn().await?;
    let tenant_id = owner.tenant_id.ok_or(AppError::Forbidden)?;
    let deleted = group::delete(&mut conn, tenant_id, id).await?;
    drop(conn);

    let response = DeleteGroupResponse {
        id: deleted.group.id,
        provider: deleted.group.provider.clone(),
        name: deleted.group.name.clone(),
        released_account_count: deleted.accounts.len(),
        released_upstream_api_key_count: deleted.upstream_api_keys.len(),
        deleted_gateway_api_key_count: deleted.deleted_gateway_api_key_count,
    };
    match deleted.group.provider.as_str() {
        gpt_model::PROVIDER => {
            ProviderResourceService::<GptMaintenance>::new(&state)
                .sync_resource_snapshots(deleted.accounts, deleted.upstream_api_keys)
                .await?;
        }
        crate::provider::claude::model::PROVIDER => {
            ProviderResourceService::<ClaudeMaintenance>::new(&state)
                .sync_resource_snapshots(deleted.accounts, deleted.upstream_api_keys)
                .await?;
        }
        _ => unreachable!("持久化 Provider 分组只允许已支持的 provider"),
    }

    Ok(Json(response))
}
