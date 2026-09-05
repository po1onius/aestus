//! 独立 WASM 插件上传及固定套件组合。上传按 Provider/插槽校验 Component ABI。
use crate::{
    api::dash::auth,
    err::{AdminResult, AppError, AppResult},
    plugin::{
        self,
        model::{NewPlugin, NewPluginSuite, PluginSlot, PluginSuiteSummary, PluginSummary},
    },
    state::AppState,
};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Multipart, Path, State},
    routing::{delete, get, put},
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use tracing::{info, warn};
use uuid::Uuid;

const MAX_WASM_BYTES: usize = 8 * 1024 * 1024;
const MAX_PLUGIN_NAME_BYTES: usize = 128;
const MAX_PLUGIN_DESCRIPTION_BYTES: usize = 1024;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/",
            get(list_plugins)
                .post(upload_plugin)
                .layer(DefaultBodyLimit::max(MAX_WASM_BYTES + 1024 * 1024)),
        )
        .route("/{id}", delete(delete_plugin))
        .route("/{id}/deletion-impact", get(plugin_deletion_impact))
        .route("/suites", get(list_suites).post(create_suite))
        .route("/suites/options", get(list_suite_options))
        .route("/suites/{id}", delete(delete_suite))
        .route("/suites/{id}/deletion-impact", get(suite_deletion_impact))
        .route("/suites/{id}/enabled", put(update_suite_enabled))
}

async fn list_plugins(
    State(state): State<AppState>,
    auth::CurrentUser(owner): auth::CurrentUser,
) -> AdminResult<Json<Vec<PluginSummary>>> {
    let scope = management_scope(&owner)?;
    let mut conn = state.db_conn().await?;
    Ok(Json(plugin::sql::list_plugins(&mut conn, scope).await?))
}

async fn list_suites(
    State(state): State<AppState>,
    auth::CurrentUser(owner): auth::CurrentUser,
) -> AdminResult<Json<Vec<PluginSuiteSummary>>> {
    let scope = management_scope(&owner)?;
    let mut conn = state.db_conn().await?;
    Ok(Json(plugin::sql::list(&mut conn, scope).await?))
}

async fn list_suite_options(
    State(state): State<AppState>,
    auth::CurrentUser(user): auth::CurrentUser,
) -> AppResult<Json<Vec<PluginSuiteSummary>>> {
    let mut conn = state.db_conn().await?;
    Ok(Json(
        plugin::sql::list_enabled_options(
            &mut conn,
            Some(user.tenant_id.ok_or(AppError::Forbidden)?),
        )
        .await?,
    ))
}

async fn upload_plugin(
    State(state): State<AppState>,
    auth::CurrentUser(owner): auth::CurrentUser,
    mut multipart: Multipart,
) -> AdminResult<Json<PluginSummary>> {
    let tenant_id = management_scope(&owner)?;
    let mut name = String::new();
    let mut description = String::new();
    let mut provider = String::new();
    let mut slot = String::new();
    let mut wasm_bytes = Vec::new();
    let mut seen = HashSet::new();
    while let Some(field) = multipart.next_field().await.map_err(multipart_error)? {
        let field_name = field.name().unwrap_or_default().to_owned();
        if !seen.insert(field_name.clone()) {
            return Err(AppError::BadRequest {
                message: format!("重复上传字段: {field_name}"),
            }
            .into());
        }
        match field_name.as_str() {
            "name" => name = field.text().await.map_err(multipart_error)?,
            "description" => description = field.text().await.map_err(multipart_error)?,
            "provider" => provider = field.text().await.map_err(multipart_error)?,
            "slot" => slot = field.text().await.map_err(multipart_error)?,
            "wasm_file" => wasm_bytes = field.bytes().await.map_err(multipart_error)?.to_vec(),
            _ => {
                return Err(AppError::BadRequest {
                    message: format!("不支持的上传字段: {field_name}"),
                }
                .into());
            }
        }
    }
    let name = normalize_text(name, "name", MAX_PLUGIN_NAME_BYTES, false)?;
    let description = normalize_text(
        description,
        "description",
        MAX_PLUGIN_DESCRIPTION_BYTES,
        true,
    )?;
    let provider = normalize_provider(provider)?;
    let slot = PluginSlot::parse(&slot).ok_or_else(|| AppError::BadRequest {
        message: "slot 必须为 request、buffered_response 或 stream_response".to_owned(),
    })?;
    if wasm_bytes.is_empty() || wasm_bytes.len() > MAX_WASM_BYTES {
        return Err(AppError::BadRequest {
            message: "WASM 文件大小必须为 1 B 到 8 MiB".to_owned(),
        }
        .into());
    }
    let runtime = state.plugin_runtime().clone();
    let compile_provider = provider.clone();
    let (wasm_bytes, component) = tokio::task::spawn_blocking(move || {
        let component = runtime.compile(&compile_provider, slot, &wasm_bytes)?;
        Ok::<_, AppError>((wasm_bytes, component))
    }).await.map_err(|e| AppError::Plugin { message: format!("插件编译任务失败: {e}") })?
        .map_err(|e| {
            warn!(admin_user_id = %owner.id, tenant_id = ?tenant_id, %provider, slot = slot.as_str(), error = %e, "拒绝无效 WASM 插件上传");
            AppError::BadRequest { message: e.to_string() }
        })?;
    let wasm_sha256 = hex::encode(Sha256::digest(&wasm_bytes));
    let mut conn = state.db_conn().await?;
    let summary = plugin::sql::create_plugin(
        &mut conn,
        NewPlugin {
            tenant_id,
            provider,
            slot: slot.as_str().to_owned(),
            name,
            description,
            wasm_sha256: wasm_sha256.clone(),
            wasm_size: wasm_bytes.len() as i64,
            wasm_bytes,
            created_by: owner.id,
        },
    )
    .await?;
    // 持久化已成功，预热失败只影响下次请求是否需要编译，不把上传误报为失败。
    if let Err(error) = state
        .plugin_runtime()
        .cache_component(summary.id, wasm_sha256, component)
    {
        warn!(plugin_id = %summary.id, error = %error, "插件上传成功但编译缓存预热失败");
    }
    info!(admin_user_id = %owner.id, plugin_id = %summary.id, "插件管理者 上传插件成功");
    Ok(Json(summary))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateSuiteRequest {
    name: String,
    #[serde(default)]
    description: String,
    provider: String,
    request_plugin_id: Option<Uuid>,
    buffered_response_plugin_id: Option<Uuid>,
    stream_response_plugin_id: Option<Uuid>,
}

async fn create_suite(
    State(state): State<AppState>,
    auth::CurrentUser(owner): auth::CurrentUser,
    Json(payload): Json<CreateSuiteRequest>,
) -> AdminResult<Json<PluginSuiteSummary>> {
    let mut conn = state.db_conn().await?;
    let suite = plugin::sql::create_suite(
        &mut conn,
        NewPluginSuite {
            tenant_id: management_scope(&owner)?,
            created_by: owner.id,
            name: normalize_text(payload.name, "name", MAX_PLUGIN_NAME_BYTES, false)?,
            description: normalize_text(
                payload.description,
                "description",
                MAX_PLUGIN_DESCRIPTION_BYTES,
                true,
            )?,
            provider: normalize_provider(payload.provider)?,
            request_plugin_id: payload.request_plugin_id,
            buffered_response_plugin_id: payload.buffered_response_plugin_id,
            stream_response_plugin_id: payload.stream_response_plugin_id,
        },
    )
    .await?;
    info!(admin_user_id = %owner.id, plugin_suite_id = %suite.id, "插件管理者 创建固定插件套件成功");
    Ok(Json(suite))
}

async fn delete_plugin(
    State(state): State<AppState>,
    auth::CurrentUser(owner): auth::CurrentUser,
    Path(id): Path<Uuid>,
) -> AdminResult<Json<plugin::sql::DeletePluginResponse>> {
    let mut conn = state.db_conn().await?;
    let deleted = plugin::sql::delete_plugin(&mut conn, management_scope(&owner)?, id).await?;
    info!(admin_user_id = %owner.id, plugin_id = %id, "插件管理者 删除插件成功");
    Ok(Json(deleted))
}

async fn delete_suite(
    State(state): State<AppState>,
    auth::CurrentUser(owner): auth::CurrentUser,
    Path(id): Path<Uuid>,
) -> AdminResult<Json<plugin::sql::DeletePluginResponse>> {
    let mut conn = state.db_conn().await?;
    let deleted = plugin::sql::delete_suite(&mut conn, management_scope(&owner)?, id).await?;
    info!(admin_user_id = %owner.id, plugin_suite_id = %id, "插件管理者 删除套件成功");
    Ok(Json(deleted))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateEnabledRequest {
    enabled: bool,
}

async fn update_suite_enabled(
    State(state): State<AppState>,
    auth::CurrentUser(owner): auth::CurrentUser,
    Path(id): Path<Uuid>,
    Json(payload): Json<UpdateEnabledRequest>,
) -> AdminResult<Json<Vec<PluginSuiteSummary>>> {
    let tenant_id = management_scope(&owner)?;
    let mut conn = state.db_conn().await?;
    plugin::sql::set_enabled(&mut conn, tenant_id.clone(), id, payload.enabled).await?;
    info!(admin_user_id = %owner.id, plugin_suite_id = %id, enabled = payload.enabled, "插件管理者 更新套件状态");
    Ok(Json(plugin::sql::list(&mut conn, tenant_id).await?))
}

/// 管理权限不等同于资源可见性。平台只能写公共资源，owner 只能写自己租户。
/// 不扩大通用 AdminUser 的权限，避免平台管理员因此获得其他租户管理接口的写入权。
fn management_scope(user: &crate::user::User) -> AppResult<Option<String>> {
    if user.is_platform_admin() {
        Ok(None)
    } else if user.is_tenant_owner() {
        Ok(Some(user.tenant_id.clone().ok_or(AppError::Forbidden)?))
    } else {
        warn!(user_id = %user.id, role = %user.role, "非插件管理者访问插件写入或管理列表");
        Err(AppError::Forbidden)
    }
}

async fn plugin_deletion_impact(
    State(state): State<AppState>,
    auth::CurrentUser(user): auth::CurrentUser,
    Path(id): Path<Uuid>,
) -> AdminResult<Json<plugin::sql::DeletionImpact>> {
    let scope = management_scope(&user)?;
    let mut conn = state.db_conn().await?;
    Ok(Json(
        plugin::sql::deletion_impact(&mut conn, scope, id, false).await?,
    ))
}

async fn suite_deletion_impact(
    State(state): State<AppState>,
    auth::CurrentUser(user): auth::CurrentUser,
    Path(id): Path<Uuid>,
) -> AdminResult<Json<plugin::sql::DeletionImpact>> {
    let scope = management_scope(&user)?;
    let mut conn = state.db_conn().await?;
    Ok(Json(
        plugin::sql::deletion_impact(&mut conn, scope, id, true).await?,
    ))
}

fn normalize_provider(provider: String) -> AppResult<String> {
    let provider = provider.trim().to_ascii_lowercase();
    if matches!(
        provider.as_str(),
        plugin::PROVIDER_GPT | plugin::PROVIDER_CLAUDE
    ) {
        return Ok(provider);
    }
    Err(AppError::BadRequest {
        message: format!("provider 仅支持 gpt 或 claude: {provider}"),
    })
}

fn normalize_text(
    value: String,
    field: &str,
    max_bytes: usize,
    allow_empty: bool,
) -> AppResult<String> {
    let value = value.trim().to_owned();
    if !allow_empty && value.is_empty() {
        return Err(AppError::BadRequest {
            message: format!("{field} 不能为空"),
        });
    }
    if value.len() > max_bytes {
        return Err(AppError::BadRequest {
            message: format!("{field} 不能超过 {max_bytes} 字节"),
        });
    }
    Ok(value)
}

fn multipart_error(source: axum::extract::multipart::MultipartError) -> AppError {
    AppError::BadRequest {
        message: format!("读取插件 multipart 上传失败: {source}"),
    }
}
