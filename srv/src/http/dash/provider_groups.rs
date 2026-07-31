use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{get, post, put},
};
use serde::Deserialize;
use uuid::Uuid;

use crate::{
    err::{AdminResult, AppResult},
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

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/options", get(list_group_options))
        .route("/unassigned-resources", get(list_unassigned_resources))
        .route("/", get(list_groups).post(create_group))
        .route("/{id}", put(rename_group))
        .route("/{id}/models", put(update_group_models))
        .route("/{id}/enabled", post(update_group_enabled))
}

async fn list_unassigned_resources(
    State(state): State<AppState>,
    _admin: auth::AdminUser,
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
    Ok(Json(
        group::list_unassigned_resources(&mut conn, provider).await?,
    ))
}

/// 分组名称不是凭证信息。普通用户创建网关 Key 时需要读取启用分组，因此这里使用
/// CurrentUser；管理统计与所有写操作仍严格要求 AdminUser。
async fn list_group_options(
    State(state): State<AppState>,
    _current_user: auth::CurrentUser,
) -> AppResult<Json<Vec<ProviderGroupWithModels>>> {
    let mut conn = state.db_conn().await?;
    Ok(Json(group::list_enabled(&mut conn).await?))
}

async fn list_groups(
    State(state): State<AppState>,
    _admin: auth::AdminUser,
    Query(query): Query<GroupListQuery>,
) -> AdminResult<Json<Vec<ProviderGroupSummary>>> {
    let provider = query
        .provider
        .as_deref()
        .map(str::trim)
        .filter(|provider| !provider.is_empty());
    let mut conn = state.db_conn().await?;
    Ok(Json(group::list_summaries(&mut conn, provider).await?))
}

async fn create_group(
    State(state): State<AppState>,
    _admin: auth::AdminUser,
    Json(payload): Json<CreateGroupRequest>,
) -> AdminResult<Json<ProviderGroupWithModels>> {
    let mut conn = state.db_conn().await?;
    let created = group::create(
        &mut conn,
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
                .sync_group_resources(created.accounts, created.api_keys)
                .await?;
        }
        crate::provider::claude::model::PROVIDER => {
            ProviderResourceService::<ClaudeMaintenance>::new(&state)
                .sync_group_resources(created.accounts, created.api_keys)
                .await?;
        }
        _ => unreachable!("group::create 已验证 provider"),
    }

    Ok(Json(created.group))
}

async fn rename_group(
    State(state): State<AppState>,
    _admin: auth::AdminUser,
    Path(id): Path<Uuid>,
    Json(payload): Json<RenameGroupRequest>,
) -> AdminResult<Json<ProviderGroupWithModels>> {
    let mut conn = state.db_conn().await?;
    let group = group::rename(&mut conn, id, payload.name).await?;
    Ok(Json(group::with_models(&mut conn, group).await?))
}

async fn update_group_enabled(
    State(state): State<AppState>,
    _admin: auth::AdminUser,
    Path(id): Path<Uuid>,
    Json(payload): Json<UpdateGroupEnabledRequest>,
) -> AdminResult<Json<ProviderGroupWithModels>> {
    let mut conn = state.db_conn().await?;
    let group = group::update_enabled(&mut conn, id, payload.enabled).await?;
    Ok(Json(group::with_models(&mut conn, group).await?))
}

async fn update_group_models(
    State(state): State<AppState>,
    _admin: auth::AdminUser,
    Path(id): Path<Uuid>,
    Json(payload): Json<UpdateGroupModelsRequest>,
) -> AdminResult<Json<ProviderGroupWithModels>> {
    let mut conn = state.db_conn().await?;
    Ok(Json(
        group::update_models(&mut conn, id, payload.models).await?,
    ))
}
