pub mod model;
pub mod runtime;
pub mod sql;
pub(crate) mod sse;

pub const PROVIDER_GPT: &str = "gpt";
pub const PROVIDER_CLAUDE: &str = "claude";

use std::{sync::Arc, time::Instant};

use bytes::Bytes;
use diesel_async::AsyncPgConnection;
use tracing::{info, warn};
use wasmtime::component::Component;

use crate::{
    err::{AppError, AppResult},
    state::AppState,
};

use self::{
    model::{PluginArtifactBinding, PluginBinding, PluginSlot},
    runtime::{
        BufferedPluginInput, BufferedPluginOutput, PluginResponseContext, RequestPluginInput,
        RequestPluginOutput, StreamPluginFinishOutput, StreamPluginItemOutput, StreamPluginSession,
        StreamPluginStartOutput,
    },
};

/// 在短只读快照中校验组合并取得冷缓存字节，事务结束后编译。删除不会让本次请求
/// 只取得半套组件，也不需要把数据库事务或行锁延长到上游请求/流式响应期间。
pub async fn prepare_binding(
    state: &AppState,
    conn: &mut AsyncPgConnection,
    tenant_id: String,
    suite_id: uuid::Uuid,
    provider: &str,
) -> AppResult<PluginBinding> {
    let (mut binding, cold_plugins) = conn
        .build_transaction()
        .repeatable_read()
        .read_only()
        .run::<_, AppError, _>(async |conn| {
            let suite = sql::find_enabled_suite(conn, &tenant_id, suite_id, provider)
                .await?
                .ok_or(AppError::InvalidApiKey)?;
            let ids = suite
                .slots()
                .into_iter()
                .filter_map(|(_, id)| id)
                .collect::<Vec<_>>();
            if ids.is_empty() {
                return Err(AppError::InvalidApiKey);
            }
            let plugins = sql::load_plugin_metadata(conn, suite.tenant_id.as_deref(), &ids).await?;
            let mut binding = PluginBinding {
                suite_id,
                tenant_id: tenant_id.clone(),
                provider: provider.to_owned(),
                artifacts: Vec::new(),
                components: std::collections::HashMap::new(),
            };
            let mut cold_plugins = Vec::new();
            for (slot, id) in suite.slots() {
                let Some(id) = id else {
                    continue;
                };
                let plugin = plugins
                    .iter()
                    .find(|p| p.id == id && p.provider == provider && p.slot == slot.as_str())
                    .ok_or(AppError::InvalidApiKey)?;
                let artifact = PluginArtifactBinding {
                    id,
                    slot,
                    wasm_sha256: plugin.wasm_sha256.clone(),
                };
                if let Some(component) = state.plugin_runtime().cached_component(&artifact)? {
                    binding.components.insert(id, component);
                } else {
                    let bytes = sql::load_wasm(conn, suite.tenant_id.as_deref(), id).await?;
                    cold_plugins.push((artifact.clone(), bytes));
                }
                binding.artifacts.push(artifact);
            }
            Ok((binding, cold_plugins))
        })
        .await?;
    for (artifact, bytes) in cold_plugins {
        let runtime = state.plugin_runtime().clone();
        let provider = binding.provider.clone();
        let slot = artifact.slot;
        let component =
            tokio::task::spawn_blocking(move || runtime.compile(&provider, slot, &bytes))
                .await
                .map_err(|e| AppError::Plugin {
                    message: format!("插件编译任务失败: {e}"),
                })??;
        state.plugin_runtime().cache_component(
            artifact.id,
            artifact.wasm_sha256.clone(),
            component.clone(),
        )?;
        binding.components.insert(artifact.id, component);
        info!(plugin_suite_id = %suite_id, plugin_id = %artifact.id, plugin_slot = slot.as_str(), "请求准备阶段完成插件冷编译");
    }
    info!(%tenant_id, plugin_suite_id = %suite_id, plugin_count = binding.artifacts.len(), "请求已持有全部插件组件，后续执行使用固定组合");
    Ok(binding)
}

pub async fn execute_request(
    state: &AppState,
    binding: &PluginBinding,
    input: RequestPluginInput,
) -> AppResult<RequestPluginOutput> {
    let artifact = require_artifact(binding, PluginSlot::Request)?;
    let component = component_for_artifact(binding, artifact)?;
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
    let component = component_for_artifact(binding, artifact)?;
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
    response_context: Option<PluginResponseContext>,
) -> AppResult<StreamPluginStartOutput> {
    let artifact = require_artifact(binding, PluginSlot::StreamResponse)?;
    let component = component_for_artifact(binding, artifact)?;
    let runtime = state.plugin_runtime().clone();
    let metadata = binding_metadata(binding_ref(binding, artifact));
    let binding = binding.clone();
    let started_at = Instant::now();
    let result = tokio::task::spawn_blocking(move || {
        runtime.start_stream(&binding, &component, status, &headers, response_context)
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

fn component_for_artifact(
    binding: &PluginBinding,
    artifact: &PluginArtifactBinding,
) -> AppResult<Arc<Component>> {
    binding
        .components
        .get(&artifact.id)
        .cloned()
        .ok_or_else(|| AppError::Plugin {
            message: format!("请求未持有已配置插槽的组件: {}", artifact.id),
        })
}

fn require_artifact(
    binding: &PluginBinding,
    slot: PluginSlot,
) -> AppResult<&PluginArtifactBinding> {
    binding.artifact(slot).ok_or_else(|| AppError::Plugin {
        message: format!(
            "插件套件缺少调用方要求的 {} 插槽: {}",
            slot.as_str(),
            binding.suite_id
        ),
    })
}

struct ExecutionMetadata {
    suite_id: uuid::Uuid,
    artifact_id: uuid::Uuid,
    slot: PluginSlot,
    provider: String,
    tenant_id: String,
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
        artifact_id: artifact.id,
        slot: artifact.slot,
        provider: binding.provider.clone(),
        tenant_id: binding.tenant_id.clone(),
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
            tenant_id = %metadata.tenant_id,
            plugin_suite_id = %metadata.suite_id,
            plugin_artifact_id = %metadata.artifact_id,
            plugin_slot = metadata.slot.as_str(),
            provider = %metadata.provider,
            elapsed_micros = started_at.elapsed().as_micros(),
            output_bytes = ?output_bytes,
            "WASM 插件 artifact 执行成功"
        ),
        Some(error) => warn!(
            tenant_id = %metadata.tenant_id,
            plugin_suite_id = %metadata.suite_id,
            plugin_artifact_id = %metadata.artifact_id,
            plugin_slot = metadata.slot.as_str(),
            provider = %metadata.provider,
            elapsed_micros = started_at.elapsed().as_micros(),
            error_code = error.code(),
            error = %error,
            "WASM 插件 artifact 执行失败"
        ),
    }
}
