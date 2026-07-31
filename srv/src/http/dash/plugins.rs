//! Admin 管理 WASM 插件套件的接口。
//!
//! 每个不可变 release 原子包含 request、buffered response、stream response 三个可空
//! artifact。所有上传文件都在数据库事务前按 Provider 和插槽完成 Component ABI 实例化；
//! 任一 artifact 无效时整套发布失败，不会留下部分版本。

use std::{collections::HashMap, sync::Arc};

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Multipart, Path, State},
    routing::{get, post, put},
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tracing::{info, warn};
use uuid::Uuid;
use wasmtime::component::Component;

use crate::{
    err::{AdminResult, AppError, AppResult},
    http::dash::auth,
    plugin::{
        self,
        model::{PluginArtifactUpload, PluginReleaseSummary, PluginSlot},
    },
    state::AppState,
};

const MAX_WASM_BYTES: usize = 8 * 1024 * 1024;
const MAX_PLUGIN_UPLOAD_BODY_BYTES: usize = MAX_WASM_BYTES * 3 + 1024 * 1024;
const MAX_PLUGIN_NAME_BYTES: usize = 128;
const MAX_PLUGIN_DESCRIPTION_BYTES: usize = 1024;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/",
            get(list_plugins)
                .post(create_plugin)
                .layer(DefaultBodyLimit::max(MAX_PLUGIN_UPLOAD_BODY_BYTES)),
        )
        .route("/options", get(list_plugin_options))
        .route(
            "/{id}/releases",
            post(publish_release).layer(DefaultBodyLimit::max(MAX_PLUGIN_UPLOAD_BODY_BYTES)),
        )
        .route("/{id}/enabled", put(update_plugin_enabled))
}

async fn list_plugins(
    State(state): State<AppState>,
    _admin: auth::AdminUser,
) -> AdminResult<Json<Vec<PluginReleaseSummary>>> {
    let mut conn = state.db_conn().await?;
    Ok(Json(plugin::sql::list(&mut conn).await?))
}

async fn list_plugin_options(
    State(state): State<AppState>,
    _current_user: auth::CurrentUser,
) -> AppResult<Json<Vec<PluginReleaseSummary>>> {
    let mut conn = state.db_conn().await?;
    Ok(Json(plugin::sql::list_enabled_options(&mut conn).await?))
}

async fn create_plugin(
    State(state): State<AppState>,
    auth::AdminUser(admin): auth::AdminUser,
    multipart: Multipart,
) -> AdminResult<Json<PluginReleaseSummary>> {
    let upload = read_create_upload(multipart).await?;
    let compiled = compile_uploads(&state, &upload.provider, upload.artifacts).await?;
    let manifest_sha256 = plugin::manifest_sha256(compiled.iter().map(|item| &item.upload));
    let uploads = compiled
        .iter()
        .map(|item| clone_upload(&item.upload))
        .collect::<Vec<_>>();
    let mut conn = state.db_conn().await?;
    let summary = plugin::sql::create_and_publish(
        &mut conn,
        admin.id,
        upload.name,
        upload.description,
        upload.provider,
        manifest_sha256,
        uploads,
    )
    .await?;
    cache_compiled_release(&state, &summary, compiled)?;
    info!(
        admin_user_id = %admin.id,
        plugin_suite_id = %summary.suite_id,
        plugin_release_id = %summary.id,
        artifact_count = summary.artifacts.len(),
        "Admin 已加载并发布 WASM 插件套件"
    );
    Ok(Json(summary))
}

async fn publish_release(
    State(state): State<AppState>,
    auth::AdminUser(admin): auth::AdminUser,
    Path(suite_id): Path<Uuid>,
    multipart: Multipart,
) -> AdminResult<Json<PluginReleaseSummary>> {
    let artifacts = read_artifact_fields(multipart).await?;
    let mut conn = state.db_conn().await?;
    let suite = plugin::sql::find_suite(&mut conn, suite_id)
        .await?
        .ok_or_else(|| AppError::BadRequest {
            message: format!("插件套件不存在: {suite_id}"),
        })?;
    drop(conn);
    let compiled = compile_uploads(&state, &suite.provider, artifacts).await?;
    let manifest_sha256 = plugin::manifest_sha256(compiled.iter().map(|item| &item.upload));
    let uploads = compiled
        .iter()
        .map(|item| clone_upload(&item.upload))
        .collect::<Vec<_>>();
    let mut conn = state.db_conn().await?;
    let summary =
        plugin::sql::publish_release(&mut conn, suite_id, admin.id, manifest_sha256, uploads)
            .await?;
    cache_compiled_release(&state, &summary, compiled)?;
    info!(
        admin_user_id = %admin.id,
        plugin_suite_id = %suite_id,
        plugin_release_id = %summary.id,
        plugin_version = summary.version,
        artifact_count = summary.artifacts.len(),
        "Admin 已发布 WASM 插件套件新版本"
    );
    Ok(Json(summary))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdatePluginEnabledRequest {
    enabled: bool,
}

async fn update_plugin_enabled(
    State(state): State<AppState>,
    auth::AdminUser(admin): auth::AdminUser,
    Path(suite_id): Path<Uuid>,
    Json(payload): Json<UpdatePluginEnabledRequest>,
) -> AdminResult<Json<Vec<PluginReleaseSummary>>> {
    let mut conn = state.db_conn().await?;
    plugin::sql::set_enabled(&mut conn, suite_id, payload.enabled).await?;
    info!(admin_user_id = %admin.id, plugin_suite_id = %suite_id, enabled = payload.enabled, "Admin 已更新 WASM 插件套件状态");
    Ok(Json(plugin::sql::list(&mut conn).await?))
}

struct CreateUpload {
    name: String,
    description: String,
    provider: String,
    artifacts: Vec<PluginArtifactUpload>,
}

struct CompiledUpload {
    upload: PluginArtifactUpload,
    component: Arc<Component>,
}

async fn read_create_upload(mut multipart: Multipart) -> AppResult<CreateUpload> {
    let mut name = None;
    let mut description = String::new();
    let mut provider = None;
    let mut files = HashMap::<PluginSlot, Vec<u8>>::new();
    while let Some(field) = multipart.next_field().await.map_err(multipart_error)? {
        match field.name() {
            Some("name") => name = Some(field.text().await.map_err(multipart_error)?),
            Some("description") => description = field.text().await.map_err(multipart_error)?,
            Some("provider") => provider = Some(field.text().await.map_err(multipart_error)?),
            Some(field_name) => {
                let slot = upload_field_slot(field_name).ok_or_else(|| AppError::BadRequest {
                    message: format!("不支持的插件套件上传字段: {field_name}"),
                })?;
                insert_upload_file(
                    &mut files,
                    slot,
                    field.bytes().await.map_err(multipart_error)?,
                )?;
            }
            None => {}
        }
    }
    let artifacts = uploads_from_files(files)?;
    Ok(CreateUpload {
        name: normalize_text(
            name.unwrap_or_default(),
            "name",
            MAX_PLUGIN_NAME_BYTES,
            false,
        )?,
        description: normalize_text(
            description,
            "description",
            MAX_PLUGIN_DESCRIPTION_BYTES,
            true,
        )?,
        provider: normalize_provider(provider.unwrap_or_default())?,
        artifacts,
    })
}

async fn read_artifact_fields(mut multipart: Multipart) -> AppResult<Vec<PluginArtifactUpload>> {
    let mut files = HashMap::<PluginSlot, Vec<u8>>::new();
    while let Some(field) = multipart.next_field().await.map_err(multipart_error)? {
        let Some(field_name) = field.name() else {
            continue;
        };
        let Some(slot) = upload_field_slot(field_name) else {
            return Err(AppError::BadRequest {
                message: format!("新版本上传不支持字段: {field_name}"),
            });
        };
        insert_upload_file(
            &mut files,
            slot,
            field.bytes().await.map_err(multipart_error)?,
        )?;
    }
    uploads_from_files(files)
}

fn upload_field_slot(field: &str) -> Option<PluginSlot> {
    match field {
        "request_file" => Some(PluginSlot::Request),
        "buffered_response_file" => Some(PluginSlot::BufferedResponse),
        "stream_response_file" => Some(PluginSlot::StreamResponse),
        _ => None,
    }
}

fn insert_upload_file(
    files: &mut HashMap<PluginSlot, Vec<u8>>,
    slot: PluginSlot,
    bytes: axum::body::Bytes,
) -> AppResult<()> {
    if bytes.is_empty() || bytes.len() > MAX_WASM_BYTES {
        return Err(AppError::BadRequest {
            message: format!(
                "{} 插槽 WASM 文件必须为 1..={MAX_WASM_BYTES} 字节",
                slot.as_str()
            ),
        });
    }
    if files.insert(slot, bytes.to_vec()).is_some() {
        return Err(AppError::BadRequest {
            message: format!("{} 插槽重复上传", slot.as_str()),
        });
    }
    Ok(())
}

fn uploads_from_files(
    mut files: HashMap<PluginSlot, Vec<u8>>,
) -> AppResult<Vec<PluginArtifactUpload>> {
    let mut artifacts = Vec::new();
    for slot in PluginSlot::ALL {
        if let Some(wasm_bytes) = files.remove(&slot) {
            artifacts.push(PluginArtifactUpload {
                slot,
                wasm_sha256: hex::encode(Sha256::digest(&wasm_bytes)),
                wasm_bytes,
            });
        }
    }
    if artifacts.is_empty() {
        return Err(AppError::BadRequest {
            message: "插件套件至少需要上传一个 WASM Component".to_owned(),
        });
    }
    Ok(artifacts)
}

async fn compile_uploads(
    state: &AppState,
    provider: &str,
    artifacts: Vec<PluginArtifactUpload>,
) -> AppResult<Vec<CompiledUpload>> {
    let mut tasks = tokio::task::JoinSet::new();
    for upload in artifacts {
        let runtime = state.plugin_runtime().clone();
        let provider = provider.to_owned();
        tasks.spawn_blocking(move || {
            let component = runtime.compile(&provider, upload.slot, &upload.wasm_bytes)?;
            Ok::<_, AppError>(CompiledUpload { upload, component })
        });
    }
    let mut compiled = Vec::new();
    while let Some(result) = tasks.join_next().await {
        compiled.push(result.map_err(|source| AppError::BadRequest {
            message: format!("WASM 编译任务异常结束: {source}"),
        })??);
    }
    compiled.sort_by_key(|item| item.upload.slot.as_str());
    Ok(compiled)
}

fn cache_compiled_release(
    state: &AppState,
    summary: &PluginReleaseSummary,
    compiled: Vec<CompiledUpload>,
) -> AppResult<()> {
    for item in compiled {
        let Some(artifact) = summary
            .artifacts
            .iter()
            .find(|artifact| artifact.slot == item.upload.slot)
        else {
            warn!(plugin_release_id = %summary.id, plugin_slot = item.upload.slot.as_str(), "已发布套件没有返回对应 artifact，跳过预热缓存");
            continue;
        };
        state.plugin_runtime().cache_component(
            artifact.id,
            artifact.wasm_sha256.clone(),
            item.component,
        )?;
    }
    Ok(())
}

fn clone_upload(upload: &PluginArtifactUpload) -> PluginArtifactUpload {
    PluginArtifactUpload {
        slot: upload.slot,
        wasm_sha256: upload.wasm_sha256.clone(),
        wasm_bytes: upload.wasm_bytes.clone(),
    }
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
        message: format!("读取插件套件 multipart 上传失败: {source}"),
    }
}
