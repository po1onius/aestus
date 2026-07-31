pub mod model;
pub mod runtime;
pub mod sql;
pub(crate) mod sse;

pub const PROVIDER_GPT: &str = "gpt";
pub const PROVIDER_CLAUDE: &str = "claude";
pub const ABI_VERSION: i32 = 1;

use std::{sync::Arc, time::Instant};

use bytes::Bytes;
use sha2::{Digest, Sha256};
use tracing::{info, warn};
use wasmtime::component::Component;

use crate::{
    err::{AppError, AppResult},
    state::AppState,
};

use self::{
    model::{PluginArtifactBinding, PluginArtifactUpload, PluginBinding, PluginSlot},
    runtime::{
        BufferedPluginInput, BufferedPluginOutput, RequestPluginInput, RequestPluginOutput,
        StreamPluginFinishOutput, StreamPluginItemOutput, StreamPluginSession,
        StreamPluginStartOutput,
    },
};

/// 套件 manifest 只覆盖稳定 ABI、slot 和 artifact 摘要，不包含数据库 ID 或发布时间。
/// 同一套件重复上传完全相同的三个插槽时会得到相同摘要并由唯一约束拒绝重复发布。
pub fn manifest_sha256<'a>(artifacts: impl Iterator<Item = &'a PluginArtifactUpload>) -> String {
    let mut parts = artifacts
        .map(|artifact| {
            (
                artifact.slot.as_str(),
                artifact.wasm_sha256.as_str(),
                ABI_VERSION,
            )
        })
        .collect::<Vec<_>>();
    parts.sort_unstable_by_key(|part| part.0);
    let mut digest = Sha256::new();
    for (slot, sha256, abi_version) in parts {
        digest.update(slot.as_bytes());
        digest.update([0]);
        digest.update(abi_version.to_be_bytes());
        digest.update(sha256.as_bytes());
        digest.update([0]);
    }
    hex::encode(digest.finalize())
}

pub async fn execute_request(
    state: &AppState,
    binding: &PluginBinding,
    input: RequestPluginInput,
) -> AppResult<RequestPluginOutput> {
    let artifact = require_artifact(binding, PluginSlot::Request)?;
    let component = component_for_artifact(state, binding, artifact).await?;
    let runtime = state.plugin_runtime().clone();
    let metadata = binding_metadata(binding_ref(binding, artifact));
    let binding_for_task = binding.clone();
    let started_at = Instant::now();
    let result = tokio::task::spawn_blocking(move || {
        runtime.execute_request(&binding_for_task, &component, input)
    })
    .await
    .map_err(|source| AppError::Plugin {
        message: format!("请求插件 blocking 任务异常结束: {source}"),
    })?;
    log_execution(
        metadata,
        started_at,
        result.as_ref().ok().map(|output| output.body.len()),
        result.as_ref().err(),
    );
    result
}

pub async fn execute_buffered(
    state: &AppState,
    binding: &PluginBinding,
    input: BufferedPluginInput,
) -> AppResult<BufferedPluginOutput> {
    let artifact = require_artifact(binding, PluginSlot::BufferedResponse)?;
    let component = component_for_artifact(state, binding, artifact).await?;
    let runtime = state.plugin_runtime().clone();
    let binding_for_task = binding.clone();
    let metadata = binding_metadata(binding_ref(binding, artifact));
    let started_at = Instant::now();
    let result = tokio::task::spawn_blocking(move || {
        runtime.execute_buffered(&binding_for_task, &component, input)
    })
    .await
    .map_err(|source| AppError::Plugin {
        message: format!("buffered 响应插件 blocking 任务异常结束: {source}"),
    })?;
    log_execution(metadata, started_at, None, result.as_ref().err());
    result
}

pub async fn start_stream(
    state: &AppState,
    binding: &PluginBinding,
    status: axum::http::StatusCode,
    headers: axum::http::HeaderMap,
    request_context: Option<Bytes>,
) -> AppResult<StreamPluginStartOutput> {
    let artifact = require_artifact(binding, PluginSlot::StreamResponse)?;
    let component = component_for_artifact(state, binding, artifact).await?;
    let runtime = state.plugin_runtime().clone();
    let metadata = binding_metadata(binding_ref(binding, artifact));
    let binding = binding.clone();
    let started_at = Instant::now();
    let result = tokio::task::spawn_blocking(move || {
        runtime.start_stream(&binding, &component, status, &headers, request_context)
    })
    .await
    .map_err(|source| AppError::Plugin {
        message: format!("stream 响应插件初始化任务异常结束: {source}"),
    })?;
    log_execution(metadata, started_at, None, result.as_ref().err());
    result
}

/// session 在 blocking 任务中按值移动并随结果返回，保证同一 SSE 响应始终复用同一个
/// Store/Component 实例，同时不在 Tokio worker 上执行同步 WASM 代码。
pub async fn transform_stream_items(
    mut session: StreamPluginSession,
    items: Vec<Bytes>,
) -> AppResult<StreamPluginBatchOutput> {
    tokio::task::spawn_blocking(move || {
        let mut outputs = Vec::with_capacity(items.len());
        for item in items {
            match session.transform_item(item) {
                Ok(output) => outputs.push(output),
                Err(error) => {
                    return StreamPluginBatchOutput {
                        session,
                        outputs,
                        error: Some(error),
                    };
                }
            }
        }
        StreamPluginBatchOutput {
            session,
            outputs,
            error: None,
        }
    })
    .await
    .map_err(|source| AppError::Plugin {
        message: format!("stream 响应插件 item batch 任务异常结束: {source}"),
    })
}

/// 一个网络 chunk 可能同时包含多个 SSE item。批量执行中若后一个 item 失败，前面已经
/// 成功返回的 effects 仍必须交给宿主处理，不能因为批处理错误而丢失已确认的资源事实。
pub struct StreamPluginBatchOutput {
    pub session: StreamPluginSession,
    pub outputs: Vec<StreamPluginItemOutput>,
    pub error: Option<AppError>,
}

pub async fn finish_stream(
    mut session: StreamPluginSession,
) -> AppResult<StreamPluginFinishOutput> {
    tokio::task::spawn_blocking(move || session.finish())
        .await
        .map_err(|source| AppError::Plugin {
            message: format!("stream 响应插件 finish 任务异常结束: {source}"),
        })?
}

async fn component_for_artifact(
    state: &AppState,
    binding: &PluginBinding,
    artifact: &PluginArtifactBinding,
) -> AppResult<Arc<Component>> {
    if let Some(component) = state.plugin_runtime().cached_component(artifact)? {
        return Ok(component);
    }
    let mut conn = state.db_conn().await?;
    let stored = sql::load_artifact(&mut conn, binding, artifact.id).await?;
    drop(conn);
    if stored.binding != *artifact || stored.suite_binding != *binding {
        return Err(AppError::Plugin {
            message: format!("插件 artifact 元数据与鉴权快照不一致: {}", artifact.id),
        });
    }
    let runtime = state.plugin_runtime().clone();
    let provider = binding.provider.clone();
    let slot = artifact.slot;
    let wasm_bytes = stored.wasm_bytes;
    let artifact_id = artifact.id;
    let component =
        tokio::task::spawn_blocking(move || runtime.compile(&provider, slot, &wasm_bytes))
            .await
            .map_err(|source| AppError::Plugin {
                message: format!(
                    "插件 artifact 编译任务异常结束: id={artifact_id}, error={source}"
                ),
            })?
            .map_err(|error| AppError::Plugin {
                message: format!("已发布插件 artifact 无法编译: id={artifact_id}, error={error}"),
            })?;
    state.plugin_runtime().cache_component(
        artifact.id,
        artifact.wasm_sha256.clone(),
        component.clone(),
    )?;
    info!(
        plugin_release_id = %binding.release_id,
        plugin_artifact_id = %artifact.id,
        plugin_slot = artifact.slot.as_str(),
        wasm_sha256 = %artifact.wasm_sha256,
        "已发布插件 artifact 首次请求编译并写入进程缓存"
    );
    Ok(component)
}

fn require_artifact(
    binding: &PluginBinding,
    slot: PluginSlot,
) -> AppResult<&PluginArtifactBinding> {
    binding.artifact(slot).ok_or_else(|| AppError::Plugin {
        message: format!(
            "插件套件 release 缺少调用方要求的 {} 插槽: {}",
            slot.as_str(),
            binding.release_id
        ),
    })
}

struct ExecutionMetadata {
    suite_id: uuid::Uuid,
    release_id: uuid::Uuid,
    artifact_id: uuid::Uuid,
    slot: PluginSlot,
    version: i64,
    provider: String,
}

fn binding_ref<'a>(
    binding: &'a PluginBinding,
    artifact: &'a PluginArtifactBinding,
) -> (&'a PluginBinding, &'a PluginArtifactBinding) {
    (binding, artifact)
}

fn binding_metadata(
    (binding, artifact): (&PluginBinding, &PluginArtifactBinding),
) -> ExecutionMetadata {
    ExecutionMetadata {
        suite_id: binding.suite_id,
        release_id: binding.release_id,
        artifact_id: artifact.id,
        slot: artifact.slot,
        version: binding.version,
        provider: binding.provider.clone(),
    }
}

fn log_execution(
    metadata: ExecutionMetadata,
    started_at: Instant,
    output_bytes: Option<usize>,
    error: Option<&AppError>,
) {
    match error {
        None => info!(
            plugin_suite_id = %metadata.suite_id,
            plugin_release_id = %metadata.release_id,
            plugin_artifact_id = %metadata.artifact_id,
            plugin_slot = metadata.slot.as_str(),
            plugin_version = metadata.version,
            provider = %metadata.provider,
            elapsed_micros = started_at.elapsed().as_micros(),
            output_bytes = ?output_bytes,
            "WASM 插件 artifact 执行成功"
        ),
        Some(error) => warn!(
            plugin_suite_id = %metadata.suite_id,
            plugin_release_id = %metadata.release_id,
            plugin_artifact_id = %metadata.artifact_id,
            plugin_slot = metadata.slot.as_str(),
            plugin_version = metadata.version,
            provider = %metadata.provider,
            elapsed_micros = started_at.elapsed().as_micros(),
            error_code = error.code(),
            error = %error,
            "WASM 插件 artifact 执行失败"
        ),
    }
}
