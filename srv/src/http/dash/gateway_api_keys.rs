//! 调用方网关 API Key 的 Dashboard HTTP 接口。
//!
//! 该资源用于调用者访问本服务，与转发模型请求所使用的上游官方 API Key 分开管理。

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::header::CACHE_CONTROL,
    response::IntoResponse,
    routing::{get, post, put},
};
use serde::{Deserialize, Serialize};
use tracing::info;
use uuid::Uuid;

use crate::{
    api_key::{self, ApiKeyWithModels},
    err::{AppError, AppResult},
    http::dash::{auth, pagination::ListPageQuery},
    plugin::{self, model::PluginReleaseSummary},
    provider::group::{self, ProviderGroup, ProviderGroupWithModels},
    state::AppState,
};

const MAX_API_KEY_NAME_BYTES: usize = 128;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateApiKeyRequest {
    name: String,
    group_id: Uuid,
    #[serde(default)]
    allowed_models: Vec<String>,
    #[serde(default)]
    plugin_release_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateApiKeyPluginRequest {
    plugin_release_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateApiKeyEnabledRequest {
    enabled: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateApiKeyModelsRequest {
    allowed_models: Vec<String>,
}

// 响应包含原始 Key，刻意不派生 Debug，避免后续错误地把完整响应写入日志。
#[derive(Serialize)]
struct ApiKeyResponse {
    id: Uuid,
    name: String,
    api_key: String,
    enabled: bool,
    group: ProviderGroup,
    group_allowed_models: Vec<String>,
    allowed_models: Vec<String>,
    plugin: Option<PluginReleaseSummary>,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
    disabled_at: Option<chrono::DateTime<chrono::Utc>>,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list_api_keys).post(create_api_key))
        .route("/{id}/plugin", put(update_api_key_plugin))
        .route("/{id}/models", put(update_api_key_models))
        .route("/{id}/enabled", post(update_api_key_enabled))
}

async fn create_api_key(
    State(state): State<AppState>,
    auth::CurrentUser(current_user): auth::CurrentUser,
    Json(payload): Json<CreateApiKeyRequest>,
) -> AppResult<impl IntoResponse> {
    let name = normalize_name(payload.name)?;
    let mut conn = state.db_conn().await?;

    let api_key = api_key::create(
        &mut conn,
        current_user.id,
        payload.group_id,
        name,
        payload.allowed_models,
        payload.plugin_release_id,
    )
    .await?;
    let group = load_group(&mut conn, api_key.api_key.group_id).await?;
    let plugin = load_plugin_summary(&mut conn, api_key.api_key.plugin_release_id).await?;

    info!(api_key_id = %api_key.api_key.id, "管理端创建 API Key 成功");

    Ok(no_store_json(ApiKeyResponse::from_parts(
        api_key, group, plugin,
    )))
}

async fn list_api_keys(
    State(state): State<AppState>,
    auth::CurrentUser(current_user): auth::CurrentUser,
    Query(query): Query<ListPageQuery>,
) -> AppResult<impl IntoResponse> {
    let page = query.normalize()?;
    let mut conn = state.db_conn().await?;

    let api_keys = api_key::list_by_user(
        &mut conn,
        current_user.id,
        page.query_limit(),
        page.offset(),
    )
    .await?;
    let group_ids = api_keys
        .iter()
        .map(|item| item.api_key.group_id)
        .collect::<Vec<_>>();
    let groups = group::find_by_ids(&mut conn, &group_ids).await?;
    let group_models = group::load_models_by_group_ids(&mut conn, &group_ids).await?;
    let plugin_release_ids = api_keys
        .iter()
        .filter_map(|item| item.api_key.plugin_release_id)
        .collect::<Vec<_>>();
    let plugins = plugin::sql::find_summaries_by_ids(&mut conn, &plugin_release_ids).await?;
    let items = api_keys
        .into_iter()
        .map(|api_key| {
            let group = groups
                .get(&api_key.api_key.group_id)
                .cloned()
                .ok_or_else(|| AppError::DbQuery {
                    message: format!(
                        "API Key 关联的 Provider 分组不存在: {}",
                        api_key.api_key.group_id
                    ),
                })?;
            let group_allowed_models = group_models
                .get(&api_key.api_key.group_id)
                .cloned()
                .ok_or_else(|| AppError::DbQuery {
                    message: format!(
                        "API Key 关联的 Provider 分组缺少模型白名单: {}",
                        api_key.api_key.group_id
                    ),
                })?;
            let plugin = api_key
                .api_key
                .plugin_release_id
                .map(|id| {
                    plugins.get(&id).cloned().ok_or_else(|| AppError::DbQuery {
                        message: format!("API Key 关联的插件发布版本不存在: {id}"),
                    })
                })
                .transpose()?;
            Ok(ApiKeyResponse::from_parts(
                api_key,
                ProviderGroupWithModels {
                    group,
                    allowed_models: group_allowed_models,
                },
                plugin,
            ))
        })
        .collect::<AppResult<Vec<_>>>()?;

    Ok(no_store_json(page.finish(items)))
}

async fn update_api_key_enabled(
    State(state): State<AppState>,
    auth::CurrentUser(current_user): auth::CurrentUser,
    Path(id): Path<Uuid>,
    Json(payload): Json<UpdateApiKeyEnabledRequest>,
) -> AppResult<impl IntoResponse> {
    let mut conn = state.db_conn().await?;

    let api_key =
        api_key::update_enabled_for_user(&mut conn, current_user.id, id, payload.enabled).await?;
    let group = load_group(&mut conn, api_key.api_key.group_id).await?;
    let plugin = load_plugin_summary(&mut conn, api_key.api_key.plugin_release_id).await?;
    info!(api_key_id = %api_key.api_key.id, user_id = %current_user.id, enabled = api_key.api_key.enabled, "管理端已修改 API Key 启用状态");
    Ok(no_store_json(ApiKeyResponse::from_parts(
        api_key, group, plugin,
    )))
}

async fn update_api_key_models(
    State(state): State<AppState>,
    auth::CurrentUser(current_user): auth::CurrentUser,
    Path(id): Path<Uuid>,
    Json(payload): Json<UpdateApiKeyModelsRequest>,
) -> AppResult<impl IntoResponse> {
    let mut conn = state.db_conn().await?;
    let api_key =
        api_key::update_models_for_user(&mut conn, current_user.id, id, payload.allowed_models)
            .await?;
    let group = load_group(&mut conn, api_key.api_key.group_id).await?;
    let plugin = load_plugin_summary(&mut conn, api_key.api_key.plugin_release_id).await?;
    info!(api_key_id = %api_key.api_key.id, user_id = %current_user.id, model_count = api_key.allowed_models.len(), "管理端已修改 API Key 模型白名单");
    Ok(no_store_json(ApiKeyResponse::from_parts(
        api_key, group, plugin,
    )))
}

async fn update_api_key_plugin(
    State(state): State<AppState>,
    auth::CurrentUser(current_user): auth::CurrentUser,
    Path(id): Path<Uuid>,
    Json(payload): Json<UpdateApiKeyPluginRequest>,
) -> AppResult<impl IntoResponse> {
    let mut conn = state.db_conn().await?;
    let api_key =
        api_key::update_plugin_for_user(&mut conn, current_user.id, id, payload.plugin_release_id)
            .await?;
    let group = load_group(&mut conn, api_key.api_key.group_id).await?;
    let plugin = load_plugin_summary(&mut conn, api_key.api_key.plugin_release_id).await?;

    info!(
        api_key_id = %api_key.api_key.id,
        user_id = %current_user.id,
        plugin_release_id = ?api_key.api_key.plugin_release_id,
        "管理端已修改 API Key 插件绑定"
    );

    Ok(no_store_json(ApiKeyResponse::from_parts(
        api_key, group, plugin,
    )))
}

/// API Key 管理响应包含可直接使用的原始凭证，禁止浏览器或中间代理持久缓存。
fn no_store_json<T: Serialize>(payload: T) -> impl IntoResponse {
    ([(CACHE_CONTROL, "private, no-store")], Json(payload))
}

fn normalize_name(name: String) -> AppResult<String> {
    let name = name.trim().to_owned();
    if name.is_empty() {
        return Err(AppError::BadRequest {
            message: "name 不能为空".to_owned(),
        });
    }
    if name.len() > MAX_API_KEY_NAME_BYTES {
        return Err(AppError::BadRequest {
            message: format!("name 不能超过 {MAX_API_KEY_NAME_BYTES} 字节"),
        });
    }

    Ok(name)
}

impl ApiKeyResponse {
    fn from_parts(
        api_key: ApiKeyWithModels,
        group: ProviderGroupWithModels,
        plugin: Option<PluginReleaseSummary>,
    ) -> Self {
        let ApiKeyWithModels {
            api_key,
            allowed_models,
        } = api_key;
        let ProviderGroupWithModels {
            group,
            allowed_models: group_allowed_models,
        } = group;
        Self {
            id: api_key.id,
            name: api_key.name,
            api_key: api_key.api_key,
            enabled: api_key.enabled,
            group,
            group_allowed_models,
            allowed_models,
            plugin,
            created_at: api_key.created_at,
            updated_at: api_key.updated_at,
            disabled_at: api_key.disabled_at,
        }
    }
}

async fn load_plugin_summary(
    conn: &mut diesel_async::AsyncPgConnection,
    release_id: Option<Uuid>,
) -> AppResult<Option<PluginReleaseSummary>> {
    let Some(release_id) = release_id else {
        return Ok(None);
    };
    plugin::sql::find_summaries_by_ids(conn, &[release_id])
        .await?
        .remove(&release_id)
        .ok_or_else(|| AppError::DbQuery {
            message: format!("API Key 关联的插件发布版本不存在: {release_id}"),
        })
        .map(Some)
}

async fn load_group(
    conn: &mut diesel_async::AsyncPgConnection,
    group_id: Uuid,
) -> AppResult<ProviderGroupWithModels> {
    let group = group::find_by_id(conn, group_id)
        .await?
        .ok_or_else(|| AppError::DbQuery {
            message: format!("API Key 关联的 Provider 分组不存在: {group_id}"),
        })?;
    group::with_models(conn, group).await
}
