use std::collections::HashSet;

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{delete, get, put},
};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};
use uuid::Uuid;

use crate::{
    api::dash::{
        auth as dash_auth,
        pagination::{ListPage, ListPageQuery},
    },
    err::{AdminResult, AppError, AppResult},
    provider::{
        group::ProviderGroup,
        maintenance::MaintenanceProvider,
        resource::RequestOverride,
        runtime::ApiKeyRuntimeView,
        service::{ApiKeySnapshot, ProviderResourceService},
    },
    state::AppState,
    user::group_access::{self, GroupPermission},
};

const MAX_UPSTREAM_API_KEY_BYTES: usize = 4 * 1024;
const MAX_UPSTREAM_BASE_URL_BYTES: usize = 2 * 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateProviderUpstreamApiKeyRequest {
    api_key: String,
    base_url: String,
    #[serde(default, rename = "override")]
    override_: RequestOverride,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateProviderUpstreamApiKeyEnabledRequest {
    enabled: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateRequestOverrideRequest {
    #[serde(rename = "override")]
    override_: RequestOverride,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateProviderGroupRequest {
    group_id: Option<Uuid>,
}

#[derive(Debug, Serialize)]
struct DeleteProviderUpstreamApiKeyResponse {
    id: Uuid,
}

/// GPT 与 Claude 官方 API Key 拥有完全相同的管理端 DTO；provider 名称由路由决定，
/// 不接受调用方在 body 中提交，避免跨 provider 写入或操作资源。
#[derive(Debug, Serialize)]
struct ProviderUpstreamApiKeyResponse {
    id: Uuid,
    masked_api_key: String,
    base_url: String,
    enabled: bool,
    group: Option<ProviderGroup>,
    error: Option<String>,
    #[serde(rename = "override")]
    override_: Option<RequestOverride>,
    runtime: ApiKeyRuntimeView,
}

pub fn router<P: MaintenanceProvider>() -> Router<AppState> {
    Router::new()
        .route(
            "/",
            get(list_provider_upstream_api_keys::<P>).post(create_provider_upstream_api_key::<P>),
        )
        .route("/{id}", delete(delete_provider_upstream_api_key::<P>))
        .route(
            "/{id}/enabled",
            put(update_provider_upstream_api_key_enabled::<P>),
        )
        .route(
            "/{id}/override",
            put(update_provider_upstream_api_key_override::<P>),
        )
        .route(
            "/{id}/group",
            put(update_provider_upstream_api_key_group::<P>),
        )
}

async fn create_provider_upstream_api_key<P: MaintenanceProvider>(
    State(state): State<AppState>,
    dash_auth::AdminUser(owner): dash_auth::AdminUser,
    Json(payload): Json<CreateProviderUpstreamApiKeyRequest>,
) -> AdminResult<Json<ProviderUpstreamApiKeyResponse>> {
    let api_key = normalize_api_key(payload.api_key)?;
    let base_url = normalize_base_url(payload.base_url)?;
    payload.override_.validate()?;
    let tenant_id = owner.tenant_id.clone().ok_or(AppError::Forbidden)?;
    let snapshot = ProviderResourceService::<P>::new(&state)
        .create_api_key(tenant_id, api_key, base_url, payload.override_)
        .await?;

    info!(
        provider = P::NAME,
        provider_api_key_id = %snapshot.api_key.id,
        "管理端创建未分组 provider 官方 API Key 成功，已同步统一 maintenance/runtime 状态"
    );
    Ok(Json(ProviderUpstreamApiKeyResponse::from_snapshot(
        snapshot, true,
    )?))
}

async fn list_provider_upstream_api_keys<P: MaintenanceProvider>(
    State(state): State<AppState>,
    dash_auth::CurrentUser(current_user): dash_auth::CurrentUser,
    Query(query): Query<ListPageQuery>,
) -> AppResult<Json<ListPage<ProviderUpstreamApiKeyResponse>>> {
    let page = query.normalize()?;
    let tenant_id = current_user.tenant_id.clone().ok_or(AppError::Forbidden)?;
    let mut conn = state.db_conn().await?;
    let visible_group_ids = group_access::group_ids_with_permission(
        &mut conn,
        &current_user,
        GroupPermission::OfficialApiKeyView,
    )
    .await?;
    let override_group_ids = group_access::group_ids_with_permission(
        &mut conn,
        &current_user,
        GroupPermission::OfficialApiKeyOverrideView,
    )
    .await?
    .map(|ids| ids.into_iter().collect::<HashSet<_>>());
    drop(conn);
    let service = ProviderResourceService::<P>::new(&state);
    let snapshots = match visible_group_ids {
        None => {
            service
                .list_api_keys(tenant_id, page.query_limit(), page.offset())
                .await?
        }
        Some(group_ids) => {
            service
                .list_api_keys_in_groups(tenant_id, &group_ids, page.query_limit(), page.offset())
                .await?
        }
    };
    let items = snapshots
        .into_iter()
        .map(|snapshot| {
            let can_view_override = override_group_ids.as_ref().is_none_or(|ids| {
                snapshot
                    .api_key
                    .group_id
                    .is_some_and(|id| ids.contains(&id))
            });
            ProviderUpstreamApiKeyResponse::from_snapshot(snapshot, can_view_override)
        })
        .collect::<AppResult<Vec<_>>>()?;
    Ok(Json(page.finish(items)))
}

async fn update_provider_upstream_api_key_override<P: MaintenanceProvider>(
    State(state): State<AppState>,
    dash_auth::CurrentUser(current_user): dash_auth::CurrentUser,
    Path(id): Path<Uuid>,
    Json(payload): Json<UpdateRequestOverrideRequest>,
) -> AppResult<Json<ProviderUpstreamApiKeyResponse>> {
    payload.override_.validate()?;
    let tenant_id = current_user.tenant_id.clone().ok_or(AppError::Forbidden)?;
    let service = ProviderResourceService::<P>::new(&state);
    let api_key = service
        .find_api_key(tenant_id.clone(), id)
        .await?
        .ok_or(AppError::Forbidden)?;
    let mut conn = state.db_conn().await?;
    group_access::require_permission(
        &mut conn,
        &current_user,
        api_key.group_id,
        GroupPermission::OfficialApiKeyOverrideUpdate,
    )
    .await?;
    drop(conn);
    let snapshot = if current_user.is_tenant_owner() {
        service
            .update_api_key_override(tenant_id, id, payload.override_)
            .await?
    } else {
        service
            .update_api_key_override_in_group(
                tenant_id,
                id,
                api_key.group_id.ok_or(AppError::Forbidden)?,
                payload.override_,
            )
            .await?
    };
    info!(
        actor_user_id = %current_user.id,
        provider = P::NAME,
        provider_api_key_id = %snapshot.api_key.id,
        "管理端更新 provider 官方 API Key request override 成功，runtime 已同步"
    );
    Ok(Json(ProviderUpstreamApiKeyResponse::from_snapshot(
        snapshot, true,
    )?))
}

async fn update_provider_upstream_api_key_enabled<P: MaintenanceProvider>(
    State(state): State<AppState>,
    dash_auth::AdminUser(owner): dash_auth::AdminUser,
    Path(id): Path<Uuid>,
    Json(payload): Json<UpdateProviderUpstreamApiKeyEnabledRequest>,
) -> AdminResult<Json<ProviderUpstreamApiKeyResponse>> {
    let tenant_id = owner.tenant_id.clone().ok_or(AppError::Forbidden)?;
    let snapshot = ProviderResourceService::<P>::new(&state)
        .update_api_key_enabled(tenant_id, id, payload.enabled)
        .await?;
    info!(
        provider = P::NAME,
        provider_api_key_id = %snapshot.api_key.id,
        enabled = snapshot.api_key.enabled,
        probe_pending = snapshot.api_key.next_probe_at.is_some(),
        "管理端更新 provider 官方 API Key enabled 成功，已同步 runtime"
    );
    Ok(Json(ProviderUpstreamApiKeyResponse::from_snapshot(
        snapshot, true,
    )?))
}

async fn update_provider_upstream_api_key_group<P: MaintenanceProvider>(
    State(state): State<AppState>,
    dash_auth::AdminUser(owner): dash_auth::AdminUser,
    Path(id): Path<Uuid>,
    Json(payload): Json<UpdateProviderGroupRequest>,
) -> AdminResult<Json<ProviderUpstreamApiKeyResponse>> {
    let tenant_id = owner.tenant_id.clone().ok_or(AppError::Forbidden)?;
    let snapshot = ProviderResourceService::<P>::new(&state)
        .update_api_key_group(tenant_id, id, payload.group_id)
        .await?;
    info!(
        provider = P::NAME,
        provider_api_key_id = %snapshot.api_key.id,
        provider_group_id = ?snapshot.api_key.group_id,
        "管理端调整 provider 官方 API Key 分组成功，runtime 已同步"
    );
    Ok(Json(ProviderUpstreamApiKeyResponse::from_snapshot(
        snapshot, true,
    )?))
}

async fn delete_provider_upstream_api_key<P: MaintenanceProvider>(
    State(state): State<AppState>,
    dash_auth::AdminUser(owner): dash_auth::AdminUser,
    Path(id): Path<Uuid>,
) -> AdminResult<Json<DeleteProviderUpstreamApiKeyResponse>> {
    let tenant_id = owner.tenant_id.clone().ok_or(AppError::Forbidden)?;
    let deleted = ProviderResourceService::<P>::new(&state)
        .delete_api_key(tenant_id, id)
        .await?;
    warn!(
        provider = P::NAME,
        provider_api_key_id = %deleted.id,
        "管理端删除 provider 官方 API Key，数据库和 Redis runtime 均已清理"
    );
    Ok(Json(DeleteProviderUpstreamApiKeyResponse {
        id: deleted.id,
    }))
}

fn normalize_required(value: String, field_name: &'static str) -> AppResult<String> {
    let value = value.trim().to_owned();
    if value.is_empty() {
        return Err(AppError::BadRequest {
            message: format!("{field_name} 不能为空"),
        });
    }
    Ok(value)
}

fn normalize_base_url(value: String) -> AppResult<String> {
    // Base URL 导入后不可修改，所有语义约束只在该写入边界校验一次；maintenance/runtime
    // 后续只读取已经持久化的值，不重复执行 URL 规则验证。
    let value = normalize_required(value, "base_url")?;
    if value.len() > MAX_UPSTREAM_BASE_URL_BYTES {
        return Err(AppError::BadRequest {
            message: format!("base_url 不能超过 {MAX_UPSTREAM_BASE_URL_BYTES} 字节"),
        });
    }
    let url = reqwest::Url::parse(&value).map_err(|source| AppError::BadRequest {
        message: format!("base_url 不是合法 URL: {source}"),
    })?;
    if !matches!(url.scheme(), "http" | "https") || url.host().is_none() || url.cannot_be_a_base() {
        return Err(AppError::BadRequest {
            message: "base_url 必须是包含 host 的 http 或 https URL".to_owned(),
        });
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(AppError::BadRequest {
            message: "base_url 不能包含用户名或密码".to_owned(),
        });
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(AppError::BadRequest {
            message: "base_url 不能包含 query 或 fragment".to_owned(),
        });
    }
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

fn normalize_api_key(value: String) -> AppResult<String> {
    // 不校验 `sk-`/`sk-ant-` 前缀：自定义 Base URL 可能使用不同格式，认证协议只要求
    // 可安全写入 header 的非空 ASCII 值。
    let value = normalize_required(value, "api_key")?;
    if value.len() > MAX_UPSTREAM_API_KEY_BYTES {
        return Err(AppError::BadRequest {
            message: format!("api_key 不能超过 {MAX_UPSTREAM_API_KEY_BYTES} 字节"),
        });
    }
    if !value.is_ascii() {
        return Err(AppError::BadRequest {
            message: "api_key 只能包含 ASCII 字符".to_owned(),
        });
    }
    Ok(value)
}

impl ProviderUpstreamApiKeyResponse {
    fn from_snapshot(snapshot: ApiKeySnapshot, can_view_override: bool) -> AppResult<Self> {
        let ApiKeySnapshot {
            api_key,
            group,
            mut runtime,
        } = snapshot;
        let request_override = api_key.request_override()?;
        runtime.next_probe_at = api_key.next_probe_at;
        if group.is_none() {
            runtime.runtime_ready = false;
            runtime.runtime_state = crate::provider::runtime::ApiKeyRuntimeState::NotRuntime;
        } else if api_key.next_probe_at.is_some() {
            runtime.runtime_ready = false;
            runtime.runtime_state = if api_key.enabled {
                crate::provider::runtime::ApiKeyRuntimeState::PendingProbe
            } else {
                crate::provider::runtime::ApiKeyRuntimeState::NotRuntime
            };
        } else if !api_key.enabled {
            runtime.runtime_ready = false;
            runtime.runtime_state = crate::provider::runtime::ApiKeyRuntimeState::NotRuntime;
        }
        Ok(Self {
            id: api_key.id,
            masked_api_key: mask_api_key(&api_key.api_key),
            base_url: api_key.base_url.clone(),
            enabled: api_key.enabled,
            group,
            error: api_key.error.clone(),
            override_: can_view_override.then_some(request_override),
            runtime,
        })
    }
}

fn mask_api_key(api_key: &str) -> String {
    let api_key = api_key.trim();
    let char_count = api_key.chars().count();
    if char_count <= 10 {
        return "*".repeat(char_count.max(1));
    }
    // 即使数据库中存在历史非 ASCII 数据也按字符边界掩码，管理列表不能因坏数据 panic。
    let prefix = api_key.chars().take(6).collect::<String>();
    let suffix = api_key
        .chars()
        .skip(char_count.saturating_sub(4))
        .collect::<String>();
    format!("{prefix}...{suffix}")
}
