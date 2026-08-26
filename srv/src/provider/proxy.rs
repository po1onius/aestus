use std::{
    collections::{HashSet, VecDeque},
    future::Future,
    marker::PhantomData,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};

use axum::{
    body::{Body, Bytes},
    http::{HeaderMap, Response, StatusCode, header},
};
use chrono::Utc;
use futures_util::Stream;
use tracing::{error, info, warn};

use crate::{
    err::{AppError, AppResult},
    infra::http_client::HttpClientProfile,
    plugin::{
        self, StreamPluginBatchOutput,
        model::{PluginBinding, PluginSlot},
        runtime::{
            BufferedPluginDisposition, BufferedPluginInput, PluginEffects, PluginResponseMode,
            RawPluginResponse, RequestPluginInput, StreamPluginFinishOutput,
            StreamPluginItemOutput, StreamPluginSession,
        },
    },
    provider::{
        maintenance::{self, MaintenanceProvider},
        protocol::{
            BufferedProtocolResponse, MAX_SSE_ITEM_BYTES, ProtocolFailure, ProtocolResponse,
            ProviderProtocol, ReplayableRequest, StreamErrorRecord, StreamObserver,
            StreamingProtocolResponse, TokenUsage, UpstreamAttemptContext, UpstreamFeedback,
            UpstreamRequestBodyMode, UpstreamRequestDraft, UpstreamRequestTarget,
            read_buffered_upstream_body,
        },
        resource::UpstreamResourceKind,
        response_logging::response_body_for_tracing,
        scheduler::{self, UpstreamAllocation, UpstreamLease},
    },
    request_event::{RequestEndResult, RequestEvent, StreamEndReason, UsageAttribution},
    state::AppState,
};

pub async fn execute<P>(
    state: &AppState,
    request: ReplayableRequest,
    group_id: uuid::Uuid,
    sticky_key: Option<String>,
    usage_attribution: UsageAttribution,
    plugin_binding: Option<PluginBinding>,
    plugin_original_body: Option<Bytes>,
) -> AppResult<Response<Body>>
where
    P: ProviderProtocol,
{
    // gateway 已在进入 proxy 前统一完成原始请求 inspection、模型授权和 sticky key 提取。
    // 插件只替换单次 attempt 的上游 header/body 构造段，因此必须同时具备绑定和原始 body。
    let has_request_plugin = plugin_binding
        .as_ref()
        .is_some_and(|binding| binding.artifact(PluginSlot::Request).is_some());
    if has_request_plugin != plugin_original_body.is_some() {
        return Err(AppError::Plugin {
            message: "插件绑定与原始请求体执行上下文不一致".to_owned(),
        });
    }
    let max_attempts = usize::from(state.config().upstream_retry_limit).saturating_add(1);
    // 排除集合只属于当前下游请求，不改变资源的持久健康状态。Provider 可以针对明确
    // 归因于当前 attempt 的瞬态 HTTP 错误设置 `exclude_resource_on_retry`，但这不能替代
    // 持久化的 `UpstreamFeedback`；普通传输错误是否排除也必须由各 provider adapter 决定。
    let mut excluded_resource_members = HashSet::<String>::with_capacity(max_attempts);

    info!(
        request_id = %request.request_id,
        provider = P::provider_name(),
        provider_group_id = %group_id,
        has_sticky_key = sticky_key.is_some(),
        plugin_release_id = ?plugin_binding.as_ref().map(|binding| binding.release_id),
        "provider 请求进入通用调度与重试流程"
    );

    for attempt_index in 0..max_attempts {
        let attempt_number = attempt_index + 1;
        let lease = match scheduler::acquire(
            state,
            request.request_id,
            P::provider_name(),
            group_id,
            sticky_key.as_deref(),
            &excluded_resource_members,
        )
        .await
        {
            Ok(lease) => lease,
            Err(error) => {
                warn!(
                    request_id = %request.request_id,
                    provider = P::provider_name(),
                    provider_group_id = %group_id,
                    attempt_number,
                    max_attempts,
                    error_code = error.code(),
                    error = %error,
                    "通用 pipeline 获取上游资源失败，终止请求且不复用上一轮上游错误响应"
                );
                return Err(error);
            }
        };
        let allocation = lease.allocation();
        let attempt_context = UpstreamAttemptContext {
            request_id: allocation.request_id,
            provider: P::provider_name(),
            resource_kind: allocation.resource.kind,
            resource_id: allocation.resource.id,
            runtime_revision: allocation.resource.revision,
            attempt_number,
            max_attempts,
        };

        info!(
            request_id = %allocation.request_id,
            provider = P::provider_name(),
            resource_type = allocation.resource_type(),
            resource_id = %allocation.resource.id,
            runtime_revision = allocation.resource.revision,
            attempt_number,
            max_attempts,
            "开始一次通用上游请求 attempt"
        );

        // 插件只作用于 OAuth 账号。混合分组可能在不同 attempt 间切换账号与官方 Key，
        // 因此必须在资源完成调度后逐 attempt 决定，不能在 gateway 鉴权阶段一次性决定。
        // 同一个局部 binding 同时传给请求和响应阶段，保证官方 Key 不会只绕过其中一侧。
        let attempt_plugin_binding = if allocation.resource.kind == UpstreamResourceKind::Account {
            plugin_binding.as_ref()
        } else {
            if let Some(binding) = plugin_binding.as_ref() {
                info!(
                    request_id = %allocation.request_id,
                    provider = P::provider_name(),
                    resource_type = allocation.resource_type(),
                    resource_id = %allocation.resource.id,
                    plugin_release_id = %binding.release_id,
                    attempt_number,
                    "官方 API Key attempt 跳过全部插件插槽，使用 Provider 原生请求与响应流程"
                );
            }
            None
        };

        let prepared = if let Some(binding) =
            attempt_plugin_binding.filter(|binding| binding.artifact(PluginSlot::Request).is_some())
        {
            let original_body = plugin_original_body
                .as_ref()
                .expect("插件模式在进入调度前已物化原始 body")
                .clone();
            match prepare_plugin_upstream_request::<P>(
                state,
                allocation,
                &request,
                binding,
                original_body,
            )
            .await
            {
                Ok(prepared) => prepared,
                Err(error) => {
                    warn!(
                        request_id = %allocation.request_id,
                        provider = P::provider_name(),
                        resource_type = allocation.resource_type(),
                        resource_id = %allocation.resource.id,
                        plugin_release_id = %binding.release_id,
                        attempt_number,
                        error = %error,
                        "插件构造上游请求失败，本次请求不进入网络发送且不提交资源回执"
                    );
                    lease.release().await?;
                    return Err(error);
                }
            }
        } else {
            let draft =
                match P::prepare_upstream_request(state.config(), &allocation.resource, &request) {
                    Ok(draft) => draft,
                    Err(error) => {
                        warn!(
                            request_id = %allocation.request_id,
                            provider = P::provider_name(),
                            resource_type = allocation.resource_type(),
                            resource_id = %allocation.resource.id,
                            attempt_number,
                            error = %error,
                            "provider 构造上游请求草稿失败，本次请求不进入网络发送"
                        );
                        lease.release().await?;
                        return Err(error);
                    }
                };
            match finalize_upstream_request::<P>(allocation, &request, draft).await {
                Ok(prepared) => prepared,
                Err(error) => {
                    warn!(
                        request_id = %allocation.request_id,
                        provider = P::provider_name(),
                        resource_type = allocation.resource_type(),
                        resource_id = %allocation.resource.id,
                        attempt_number,
                        error = %error,
                        "通用 pipeline 完成上游请求 override 或 provider 最终化失败，本次请求不进入网络发送"
                    );
                    lease.release().await?;
                    return Err(error);
                }
            }
        };

        let upstream_response = match send_upstream_request::<P>(state, prepared).await {
            Ok(response) => response,
            Err(error) => {
                let retry_next = attempt_number < max_attempts;
                warn!(
                    request_id = %allocation.request_id,
                    provider = P::provider_name(),
                    resource_type = allocation.resource_type(),
                    resource_id = %allocation.resource.id,
                    attempt_number,
                    max_attempts,
                    retry_next,
                    error = %error,
                    "通用 pipeline 发送上游 HTTP 请求失败；网络失败不提交资源回执"
                );
                let release_result = lease.release().await;
                release_result?;

                if retry_next {
                    continue;
                }
                return Err(retry_exhausted_resource_error::<P>(
                    group_id,
                    max_attempts,
                    format!("发送上游 HTTP 请求失败: {error}"),
                ));
            }
        };

        let (protocol_response, stream_plugin_session) = match handle_response::<P>(
            state,
            &allocation.resource,
            attempt_context,
            upstream_response,
            attempt_plugin_binding,
        )
        .await
        {
            Ok(response) => response,
            Err(failure) => {
                let network_failure = failure.is_network();
                let retry = network_failure && attempt_number < max_attempts;
                warn!(
                    request_id = %attempt_context.request_id,
                    provider = attempt_context.provider,
                    resource_type = attempt_context.resource_kind.as_str(),
                    resource_id = %attempt_context.resource_id,
                    runtime_revision = attempt_context.runtime_revision,
                    attempt_number = attempt_context.attempt_number,
                    max_attempts = attempt_context.max_attempts,
                    network_failure,
                    retry_next = retry,
                    error = %failure.error(),
                    "provider 读取或构造上游响应失败；网络接收失败由通用 executor 重试且不提交资源回执"
                );
                let error = failure.into_error();
                let release_result = lease.release().await;
                release_result?;
                if retry {
                    continue;
                }
                if network_failure {
                    return Err(retry_exhausted_resource_error::<P>(
                        group_id,
                        max_attempts,
                        format!("读取上游响应失败: {error}"),
                    ));
                }
                return Err(error);
            }
        };

        match protocol_response {
            ProtocolResponse::Buffered(BufferedProtocolResponse::Respond {
                status,
                headers,
                body,
                feedback,
                usage,
            }) => {
                let feedback_result =
                    apply_optional_feedback::<P::Maintenance>(state, allocation, feedback).await;
                if let Some(usage) = usage {
                    state.request_events().emit(RequestEvent::UsageObserved {
                        request_id: request.request_id,
                        attribution: usage_attribution,
                        usage,
                    });
                }
                release_buffered_lease::<P>(lease, status, attempt_number, max_attempts, false)
                    .await;
                feedback_result?;
                return finish_buffered_response(state, request.request_id, status, headers, body);
            }
            ProtocolResponse::Buffered(BufferedProtocolResponse::Retry {
                upstream_status,
                exclude_current_resource,
                feedback,
            }) => {
                let retry_next = attempt_number < max_attempts;
                let feedback_result =
                    apply_optional_feedback::<P::Maintenance>(state, allocation, feedback).await;
                if retry_next && exclude_current_resource {
                    exclude_resource_for_retry(
                        &mut excluded_resource_members,
                        allocation,
                        attempt_number,
                        max_attempts,
                    );
                }
                release_buffered_lease::<P>(
                    lease,
                    upstream_status,
                    attempt_number,
                    max_attempts,
                    retry_next,
                )
                .await;
                feedback_result?;

                if retry_next {
                    continue;
                }
                warn!(
                    request_id = %request.request_id,
                    provider = P::provider_name(),
                    provider_group_id = %group_id,
                    upstream_status = upstream_status.as_u16(),
                    attempt_number,
                    max_attempts,
                    "可重试上游 HTTP 错误已耗尽 attempt，不向调用方返回最后一次原始响应"
                );
                return Err(retry_exhausted_resource_error::<P>(
                    group_id,
                    max_attempts,
                    format!("最后一次可重试上游响应状态为 HTTP {upstream_status}"),
                ));
            }
            ProtocolResponse::Streaming(response) => {
                return build_streaming_response::<P::Maintenance>(
                    state.clone(),
                    lease,
                    request.request_id,
                    usage_attribution,
                    response,
                    stream_plugin_session,
                );
            }
        }
    }

    // `max_attempts` 至少为 1，且循环内的每条分支都会返回或继续；保留明确的资源错误
    // 作为防御性兜底，避免未来扩展 attempt 状态机时重新泄露最后一次上游响应。
    Err(retry_exhausted_resource_error::<P>(
        group_id,
        max_attempts,
        "重试循环结束但没有产生最终响应".to_owned(),
    ))
}

/// 把本次已经失败的资源加入请求级排除集合。
///
/// 该集合与 maintenance 回执相互独立：回执描述资源的全局持久状态，排除集合只保证当前
/// 请求的下一次 attempt 不会回到已经尝试过的账号或 API Key。resource member 同时包含
/// 资源类型与 UUID，可避免账号表和 API Key 表出现相同 UUID 时相互误排除。
fn exclude_resource_for_retry(
    excluded_resource_members: &mut HashSet<String>,
    allocation: &UpstreamAllocation,
    attempt_number: usize,
    max_attempts: usize,
) {
    let resource_member = allocation.resource_member();
    let inserted = excluded_resource_members.insert(resource_member.clone());
    let excluded_resource_count = excluded_resource_members.len();
    if inserted {
        info!(
            request_id = %allocation.request_id,
            provider = %allocation.resource.provider,
            resource_type = allocation.resource_type(),
            resource_id = %allocation.resource.id,
            runtime_revision = allocation.resource.revision,
            attempt_number,
            max_attempts,
            retry_reason = "provider_requested_resource_exclusion",
            excluded_resource_count,
            resource_member = %resource_member,
            "当前 attempt 的上游资源已加入请求级重试排除集合"
        );
    } else {
        warn!(
            request_id = %allocation.request_id,
            provider = %allocation.resource.provider,
            resource_type = allocation.resource_type(),
            resource_id = %allocation.resource.id,
            attempt_number,
            max_attempts,
            retry_reason = "provider_requested_resource_exclusion",
            excluded_resource_count,
            resource_member = %resource_member,
            "scheduler 再次分配了请求级排除集合中的资源"
        );
    }
}

/// 把“已经按策略尝试过资源但仍无法完成请求”的内部事实收口为统一资源错误。
///
/// 最后一次上游正文已经由 provider adapter 写入 tracing；这里只保留短诊断，公共响应
/// 会由 gateway 投影成 `resource_error`，不会把某个具体资源的原始失败冒充为最终结果。
fn retry_exhausted_resource_error<P: ProviderProtocol>(
    group_id: uuid::Uuid,
    attempts: usize,
    last_failure: String,
) -> AppError {
    AppError::ResourceError {
        provider: P::provider_name().to_owned(),
        group_id,
        message: format!("上游重试次数已耗尽: attempts={attempts}, last_failure={last_failure}"),
    }
}

async fn apply_optional_feedback<M: MaintenanceProvider>(
    state: &AppState,
    allocation: &UpstreamAllocation,
    feedback: Option<UpstreamFeedback>,
) -> AppResult<()> {
    let Some(feedback) = feedback else {
        return Ok(());
    };
    maintenance::apply_upstream_feedback::<M>(
        state,
        allocation.request_id,
        &allocation.resource,
        feedback,
    )
    .await?;
    Ok(())
}

/// buffered 决策已经由封闭枚举固定，lease 清理失败不能再改变“返回”或“重试”的结果。
/// 显式释放失败时 RAII guard 会使用同一 lease token 在后台幂等重试；这里保留完整 attempt
/// 诊断后继续执行既定决策，避免资源计数清理故障覆盖真实的 provider 响应语义。
async fn release_buffered_lease<P: ProviderProtocol>(
    lease: UpstreamLease,
    status: StatusCode,
    attempt_number: usize,
    max_attempts: usize,
    retry_planned: bool,
) {
    let allocation = lease.allocation();
    let request_id = allocation.request_id;
    let resource_type = allocation.resource_type();
    let resource_id = allocation.resource.id;
    let runtime_revision = allocation.resource.revision;
    if let Err(error) = lease.release().await {
        error!(
            request_id = %request_id,
            provider = P::provider_name(),
            resource_type,
            resource_id = %resource_id,
            runtime_revision,
            upstream_status = status.as_u16(),
            attempt_number,
            max_attempts,
            retry_planned,
            error = %error,
            "buffered 响应完成后释放上游资源失败；保留既定响应决策，RAII guard 已提交兜底释放"
        );
    }
}

struct PreparedUpstreamRequest {
    client_profile: HttpClientProfile,
    method: reqwest::Method,
    url: reqwest::Url,
    headers: HeaderMap,
    body: reqwest::Body,
    plugin_response_input: Option<PluginAttemptResponseInput>,
}

struct ReceivedUpstreamResponse {
    response: reqwest::Response,
    plugin_response_input: Option<PluginAttemptResponseInput>,
}

/// 请求插件为响应阶段生成的全部 attempt-local 输入。把模式与透明 context 包在同一个
/// `Option` 中，使类型直接表达“无请求插件时二者都不存在”，避免后续扩展时产生半初始化
/// 状态；该值始终和实际发出的请求一起移动，重试或切换资源不会串用上一 attempt 的数据。
struct PluginAttemptResponseInput {
    response_mode: PluginResponseMode,
    request_context: Option<Bytes>,
}

/// 插件模式只复用 provider 的目标地址选择；原生 header 白名单、资源 override 和
/// `finalize_upstream_request` 均不会执行。插件输出经过宿主的结构/容量校验后直接发送。
async fn prepare_plugin_upstream_request<P: ProviderProtocol>(
    state: &AppState,
    allocation: &UpstreamAllocation,
    request: &ReplayableRequest,
    binding: &PluginBinding,
    original_body: Bytes,
) -> AppResult<PreparedUpstreamRequest> {
    let UpstreamRequestTarget {
        client_profile,
        method,
        url,
    } = P::prepare_upstream_target(state.config(), &allocation.resource, request)?;
    let input = RequestPluginInput::from_account(
        P::provider_name(),
        allocation.resource.id,
        &allocation.resource.auth_secret,
        &allocation.resource.request_context,
        request.headers.clone(),
        original_body,
    )?;
    let output = plugin::execute_request(state, binding, input).await?;
    let url = reqwest::Url::parse(&url).map_err(|source| AppError::ProviderUpstream {
        provider: P::provider_name().to_owned(),
        message: format!("上游 URL 无效: {source}"),
    })?;

    info!(
        request_id = %allocation.request_id,
        provider = P::provider_name(),
        resource_type = allocation.resource_type(),
        resource_id = %allocation.resource.id,
        plugin_suite_id = %binding.suite_id,
        plugin_release_id = %binding.release_id,
        plugin_version = binding.version,
        upstream_url = %url,
        method = %method,
        upstream_header_count = output.headers.len(),
        upstream_body_bytes = output.body.len(),
        plugin_response_mode = output.response_mode.as_str(),
        plugin_context_present = output.response_context.is_some(),
        plugin_context_bytes = output.response_context.as_ref().map_or(0, Bytes::len),
        http_client_profile = client_profile.as_str(),
        "插件输出已成为本次 attempt 的最终上游 header/body，透明 context 已绑定到该 attempt"
    );

    Ok(PreparedUpstreamRequest {
        client_profile,
        method,
        url,
        headers: output.headers,
        body: reqwest::Body::from(output.body),
        plugin_response_input: Some(PluginAttemptResponseInput {
            response_mode: output.response_mode,
            request_context: output.response_context,
        }),
    })
}

async fn finalize_upstream_request<P: ProviderProtocol>(
    allocation: &UpstreamAllocation,
    request: &ReplayableRequest,
    mut draft: UpstreamRequestDraft,
) -> AppResult<PreparedUpstreamRequest> {
    let resource = &allocation.resource;
    let override_body_applied = !resource.request_override.body.is_empty();
    let provider_requested_body_materialization =
        matches!(&draft.body, UpstreamRequestBodyMode::MaterializeOriginal);
    let (base_body, body_source) = match draft.body {
        UpstreamRequestBodyMode::ReplayOriginal if !override_body_applied => {
            (None, "original_cache")
        }
        UpstreamRequestBodyMode::ReplayOriginal => (
            Some(request.body.replay_bytes().await?),
            "override_materialized_original_cache",
        ),
        UpstreamRequestBodyMode::MaterializeOriginal => (
            Some(request.body.replay_bytes().await?),
            "provider_materialized_original_cache",
        ),
    };

    // 通用 override 先处理两个请求要素；provider 随后在一个 hook 内同时最终化真实凭证
    // header 和可选 body attribution，确保调用方或管理员都无法覆盖实际资源身份。
    // multipart 等 operation 会先把 wire body 转成 JSON 中间表示，再进入所有 provider
    // 共用的 Merge Patch。这样管理员 override 不需要理解 multipart boundary，也不会因
    // Account/API Key 的上游编码差异失效。
    let base_body = match base_body {
        Some(body) => Some(P::transform_body_before_override(resource, request, body).await?),
        None => None,
    };
    let mut body = resource.request_override.apply(
        allocation.request_id,
        resource,
        &mut draft.headers,
        base_body,
    )?;
    P::finalize_upstream_request(
        resource,
        allocation.request_id,
        &mut draft.headers,
        body.as_mut(),
    )?;
    let body_materialized = body.is_some();
    let body = match body {
        Some(bytes) => reqwest::Body::from(bytes),
        None => request.body.replay_body().await?,
    };

    let url = reqwest::Url::parse(&draft.url).map_err(|source| AppError::ProviderUpstream {
        provider: P::provider_name().to_owned(),
        message: format!("上游 URL 无效: {source}"),
    })?;

    info!(
        request_id = %allocation.request_id,
        provider = P::provider_name(),
        resource_type = allocation.resource_type(),
        resource_id = %resource.id,
        method = %draft.method,
        upstream_url = %url,
        upstream_header_count = draft.headers.len(),
        http_client_profile = draft.client_profile.as_str(),
        body_source,
        override_body_applied,
        provider_requested_body_materialization,
        body_materialized,
        "通用 pipeline 已完成 header/body override 与 provider 单次请求最终化"
    );

    Ok(PreparedUpstreamRequest {
        client_profile: draft.client_profile,
        method: draft.method,
        url,
        headers: draft.headers,
        body,
        plugin_response_input: None,
    })
}

async fn send_upstream_request<P: ProviderProtocol>(
    state: &AppState,
    request: PreparedUpstreamRequest,
) -> AppResult<ReceivedUpstreamResponse> {
    let PreparedUpstreamRequest {
        client_profile,
        method,
        url,
        headers,
        body,
        plugin_response_input,
    } = request;
    let timeout_seconds = state.config().provider_upstream_timeout_seconds.max(1);
    let send = state
        .streaming_http_client(client_profile)
        .request(method, url)
        .headers(headers)
        .body(body)
        .send();
    let response = tokio::time::timeout(std::time::Duration::from_secs(timeout_seconds), send)
        .await
        .map_err(|_| AppError::ProviderUpstream {
            provider: P::provider_name().to_owned(),
            message: format!("等待上游响应头超时: {timeout_seconds} 秒"),
        })?
        .map_err(|source| AppError::ProviderUpstream {
            provider: P::provider_name().to_owned(),
            message: source.to_string(),
        })?;
    Ok(ReceivedUpstreamResponse {
        response,
        plugin_response_input,
    })
}

/// 成功响应优先使用请求插件声明的下游交付模式选择响应插槽；没有请求插件声明时
/// 才沿用原生 Content-Type 判断。非成功 HTTP 响应固定 buffered，确保错误正文完整交给
/// buffered 插件。命中插槽时完全绕过 provider `handle_response`；空插槽才调用原生
/// adapter，二者最终归一化为相同 ProtocolResponse。
async fn handle_response<P: ProviderProtocol>(
    state: &AppState,
    resource: &crate::provider::resource::UpstreamResource,
    attempt: UpstreamAttemptContext,
    upstream: ReceivedUpstreamResponse,
    plugin_binding: Option<&PluginBinding>,
) -> Result<(ProtocolResponse, Option<StreamPluginSession>), ProtocolFailure> {
    let ReceivedUpstreamResponse {
        response: upstream_response,
        plugin_response_input,
    } = upstream;
    let (response_mode, request_context) = match plugin_response_input {
        Some(input) => (Some(input.response_mode), input.request_context),
        None => (None, None),
    };
    let status = upstream_response.status();
    let raw_headers = upstream_response.headers().clone();
    let upstream_declares_sse = is_sse_response(&raw_headers);
    let is_stream = should_use_stream_response(status, response_mode, &raw_headers);
    let response_mode_source = if !status.is_success() {
        "http_status"
    } else if response_mode.is_some() {
        "request_plugin"
    } else {
        "content_type_fallback"
    };
    let response_slot = if is_stream {
        PluginSlot::StreamResponse
    } else {
        PluginSlot::BufferedResponse
    };

    // Content-Type 只作为无请求模式时的兼容回退和诊断字段。buffered 模式配合 SSE
    // 上游是合法场景：宿主完整收集 body 后，响应插件可将事件流转换成单个 JSON。
    if plugin_binding.is_some() {
        info!(
            request_id = %attempt.request_id,
            provider = attempt.provider,
            resource_type = attempt.resource_kind.as_str(),
            resource_id = %attempt.resource_id,
            upstream_status = status.as_u16(),
            request_plugin_response_mode = response_mode.map(PluginResponseMode::as_str),
            response_mode_source,
            upstream_declares_sse,
            selected_plugin_slot = response_slot.as_str(),
            "宿主已为本次 attempt 选择响应插件插槽"
        );
    }
    let Some(binding) = plugin_binding.filter(|binding| binding.artifact(response_slot).is_some())
    else {
        return P::handle_response(state.config(), resource, attempt, upstream_response)
            .await
            .map(|response| (response, None));
    };

    if is_stream {
        let context_bytes = request_context.as_ref().map_or(0, Bytes::len);
        let output = plugin::start_stream(state, binding, status, raw_headers, request_context)
            .await
            .map_err(ProtocolFailure::adapter)?;
        info!(
            request_id = %attempt.request_id,
            provider = attempt.provider,
            resource_type = attempt.resource_kind.as_str(),
            resource_id = %attempt.resource_id,
            plugin_release_id = %binding.release_id,
            plugin_slot = response_slot.as_str(),
            upstream_status = status.as_u16(),
            downstream_status = output.status.as_u16(),
            downstream_header_count = output.headers.len(),
            plugin_context_present = context_bytes > 0,
            plugin_context_bytes = context_bytes,
            "stream 响应插件已完全接管响应头和原始 SSE item"
        );
        return Ok((
            ProtocolResponse::Streaming(StreamingProtocolResponse {
                status: output.status,
                headers: output.headers,
                stream: Box::pin(upstream_response.bytes_stream()),
                observer: None,
            }),
            Some(output.session),
        ));
    }

    let body =
        read_buffered_upstream_body(state.config(), attempt.provider, upstream_response).await?;
    if !status.is_success() {
        let tracing_body = response_body_for_tracing(&body);
        warn!(
            request_id = %attempt.request_id,
            provider = attempt.provider,
            resource_type = attempt.resource_kind.as_str(),
            resource_id = %attempt.resource_id,
            runtime_revision = attempt.runtime_revision,
            attempt_number = attempt.attempt_number,
            max_attempts = attempt.max_attempts,
            plugin_release_id = %binding.release_id,
            upstream_status = status.as_u16(),
            response_bytes = body.len(),
            upstream_response_body_encoding = tracing_body.encoding(),
            upstream_response_body = %tracing_body.content(),
            "响应插件接管的上游 HTTP 失败正文已完整写入 tracing"
        );
    }
    let context_bytes = request_context.as_ref().map_or(0, Bytes::len);
    let output = plugin::execute_buffered(
        state,
        binding,
        BufferedPluginInput {
            response: RawPluginResponse {
                status,
                headers: raw_headers,
                body,
            },
            request_context,
        },
    )
    .await
    .map_err(ProtocolFailure::adapter)?;
    info!(
        request_id = %attempt.request_id,
        provider = attempt.provider,
        resource_type = attempt.resource_kind.as_str(),
        resource_id = %attempt.resource_id,
        plugin_release_id = %binding.release_id,
        plugin_slot = response_slot.as_str(),
        plugin_context_present = context_bytes > 0,
        plugin_context_bytes = context_bytes,
        "buffered 响应插件已消费本次 attempt 的透明请求 context"
    );
    let feedback = output.effects.feedback;
    let usage = output.effects.usage;
    let response = match output.disposition {
        BufferedPluginDisposition::Respond(response) => BufferedProtocolResponse::Respond {
            status: response.status,
            headers: response.headers,
            body: response.body,
            feedback,
            usage,
        },
        BufferedPluginDisposition::Retry {
            exclude_current_resource,
            reason,
        } => {
            if let Some(usage) = usage {
                warn!(
                    request_id = %attempt.request_id,
                    provider = attempt.provider,
                    resource_type = attempt.resource_kind.as_str(),
                    resource_id = %attempt.resource_id,
                    plugin_release_id = %binding.release_id,
                    plugin_slot = response_slot.as_str(),
                    upstream_status = status.as_u16(),
                    usage_total_tokens = usage.total_tokens,
                    retry_reason = %reason,
                    "buffered 响应插件同时声明重试和 usage，拒绝矛盾输出"
                );
                return Err(ProtocolFailure::adapter(AppError::Plugin {
                    message: "buffered 响应插件不能同时声明重试和 usage".to_owned(),
                }));
            }
            info!(
                request_id = %attempt.request_id,
                provider = attempt.provider,
                resource_type = attempt.resource_kind.as_str(),
                resource_id = %attempt.resource_id,
                plugin_release_id = %binding.release_id,
                exclude_current_resource,
                retry_reason = %reason,
                "buffered 响应插件要求通用 executor 重试"
            );
            BufferedProtocolResponse::Retry {
                upstream_status: status,
                exclude_current_resource,
                feedback,
            }
        }
    };
    Ok((ProtocolResponse::Buffered(response), None))
}

fn is_sse_response(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("text/event-stream"))
}

/// 插件声明只约束成功响应；401/429/5xx 等响应永远需要先完整读取，才能把 status、header
/// 和 body 一次性交给 buffered 插件产生 maintenance 回执、重试指令或最终错误响应。
fn should_use_stream_response(
    status: StatusCode,
    response_mode: Option<PluginResponseMode>,
    headers: &HeaderMap,
) -> bool {
    if !status.is_success() {
        return false;
    }
    match response_mode {
        Some(PluginResponseMode::Stream) => true,
        Some(PluginResponseMode::Buffered) => false,
        None => is_sse_response(headers),
    }
}

fn finish_buffered_response(
    state: &AppState,
    request_id: uuid::Uuid,
    status: StatusCode,
    headers: HeaderMap,
    body: Bytes,
) -> AppResult<Response<Body>> {
    let result = if !status.is_success() {
        RequestEndResult::HttpFailure {
            status_code: status.as_u16(),
            body: body.clone(),
        }
    } else {
        RequestEndResult::HttpSuccess
    };
    let response_started_at = Utc::now();
    // Buffered 正文已经完整确定，返回后即可由 Axum 写向调用方；这里只记录最终下游
    // 响应就绪时间，任何被重试丢弃的上游响应头都不属于客户端首字。
    if !body.is_empty() {
        state.request_events().emit(RequestEvent::ResponseStarted {
            request_id,
            occurred_at: response_started_at,
        });
    }
    // Buffered body 已完整物化，最终响应也已经确定；把结果与可选错误正文合并为一个
    // 原子终态，避免错误事实与结束事实分开发送后因部分事件丢失而误判状态。
    state.request_events().emit(RequestEvent::Ended {
        request_id,
        occurred_at: response_started_at,
        result,
    });
    let mut builder = Response::builder().status(status);
    if let Some(response_headers) = builder.headers_mut() {
        *response_headers = headers;
    }
    builder
        .body(Body::from(body))
        .map_err(|source| AppError::ProviderUpstream {
            provider: "gateway".to_owned(),
            message: format!("构造 buffered 下游响应失败: {source}"),
        })
}

fn build_streaming_response<M: MaintenanceProvider>(
    state: AppState,
    lease: UpstreamLease,
    request_id: uuid::Uuid,
    usage_attribution: UsageAttribution,
    response: StreamingProtocolResponse,
    plugin_session: Option<StreamPluginSession>,
) -> AppResult<Response<Body>> {
    let mut builder = Response::builder().status(response.status);
    if let Some(headers) = builder.headers_mut() {
        *headers = response.headers;
    }
    let stream = ManagedUpstreamStream::<M>::new(
        state,
        lease,
        request_id,
        usage_attribution,
        response.stream,
        response.observer,
        plugin_session,
    );
    builder
        .body(Body::from_stream(stream))
        .map_err(|source| AppError::ProviderUpstream {
            provider: M::NAME.to_owned(),
            message: source.to_string(),
        })
}

/// feedback future 与下游输出彻底解耦。poll 状态机仍会在输出相关 item 前等待隔离完成；
/// Drop 收尾则可以独立接管 future，避免客户端取消同时丢失已经确认的资源事实。
type PendingFeedbackSubmission = Pin<Box<dyn Future<Output = ()> + Send>>;

enum PendingPluginResult {
    Items(StreamPluginBatchOutput),
    Finish(StreamPluginFinishOutput),
}

type PendingPluginWork = Pin<Box<dyn Future<Output = AppResult<PendingPluginResult>> + Send>>;

/// 通用流包装器向 axum body 暴露的错误。
///
/// reqwest 读取错误、网关空闲超时和插件失败统一成一个错误类型，才能在不伪造
/// Provider SSE 事件的前提下终止 HTTP body，并让 Drop 收尾逻辑可靠释放 lease。
#[derive(Debug, thiserror::Error)]
enum ManagedStreamError {
    #[error("读取上游 SSE 字节流失败: {0}")]
    Upstream(#[source] reqwest::Error),
    #[error("上游 SSE 连续 {timeout_seconds} 秒未返回数据")]
    IdleTimeout { timeout_seconds: u64 },
    #[error("流式响应插件失败: {0}")]
    Plugin(String),
}

struct ManagedUpstreamStream<M: MaintenanceProvider> {
    state: AppState,
    lease: Option<UpstreamLease>,
    request_id: uuid::Uuid,
    usage_attribution: UsageAttribution,
    stream: futures_util::stream::BoxStream<'static, Result<Bytes, reqwest::Error>>,
    observer: Option<Box<dyn StreamObserver>>,
    plugin_session: Option<StreamPluginSession>,
    plugin_sse_buffer: Vec<u8>,
    pending_plugin_work: Option<PendingPluginWork>,
    plugin_upstream_eof: bool,
    output: VecDeque<Bytes>,
    pending_feedback_submission: Option<PendingFeedbackSubmission>,
    stream_idle_timeout: Option<Duration>,
    stream_idle_sleep: Option<Pin<Box<tokio::time::Sleep>>>,
    pending_stream_error: Option<ManagedStreamError>,
    finish_after_output: Option<StreamEndReason>,
    response_started_emitted: bool,
    feedback_submitted: bool,
    usage: Option<TokenUsage>,
    stream_error: Option<StreamErrorRecord>,
    marker: PhantomData<M>,
}

// 所有需要保持固定地址的异步对象都位于各自的 `Pin<Box<_>>` 中；移动外层状态机不会
// 移动这些对象。显式声明 Unpin 可避免 `PhantomData<M>` 把 provider 类型的自动 trait
// 约束错误传播到通用流包装器。
impl<M: MaintenanceProvider> Unpin for ManagedUpstreamStream<M> {}

impl<M: MaintenanceProvider> ManagedUpstreamStream<M> {
    fn new(
        state: AppState,
        lease: UpstreamLease,
        request_id: uuid::Uuid,
        usage_attribution: UsageAttribution,
        stream: futures_util::stream::BoxStream<'static, Result<Bytes, reqwest::Error>>,
        observer: Option<Box<dyn StreamObserver>>,
        plugin_session: Option<StreamPluginSession>,
    ) -> Self {
        let stream_idle_timeout = match state.config().provider_upstream_stream_idle_timeout_seconds
        {
            0 => None,
            seconds => Some(Duration::from_secs(seconds)),
        };
        // timer 从确认真实 SSE 响应后开始计时；之后每收到一批上游字节都会重置。
        let stream_idle_sleep = stream_idle_timeout.map(tokio::time::sleep).map(Box::pin);

        Self {
            state,
            lease: Some(lease),
            request_id,
            usage_attribution,
            stream,
            observer,
            plugin_session,
            plugin_sse_buffer: Vec::with_capacity(8192),
            pending_plugin_work: None,
            plugin_upstream_eof: false,
            output: VecDeque::new(),
            pending_feedback_submission: None,
            stream_idle_timeout,
            stream_idle_sleep,
            pending_stream_error: None,
            finish_after_output: None,
            response_started_emitted: false,
            feedback_submitted: false,
            usage: None,
            stream_error: None,
            marker: PhantomData,
        }
    }

    /// 上游每产生一批字节就重新计算 deadline；下游消费速度不会改变配置值。
    fn reset_stream_idle_deadline(&mut self) {
        let (Some(timeout), Some(sleep)) =
            (self.stream_idle_timeout, self.stream_idle_sleep.as_mut())
        else {
            return;
        };
        let now = tokio::time::Instant::now();
        if let Some(deadline) = now.checked_add(timeout) {
            sleep.as_mut().reset(deadline);
        } else {
            // 极端大的 u64 秒数无法表示为 Instant 时，Tokio 的 sleep 会使用其可表示的
            // 最远 deadline。这里保持与首次构造 timer 相同的安全行为，避免整数溢出。
            self.stream_idle_sleep = Some(Box::pin(tokio::time::sleep(timeout)));
        }
    }

    fn handle_stream_idle_timeout(&mut self) {
        let Some(timeout) = self.stream_idle_timeout else {
            return;
        };
        let timeout_seconds = timeout.as_secs();

        if let Some(allocation) = self.lease.as_ref().map(UpstreamLease::allocation) {
            warn!(
                request_id = %self.request_id,
                provider = M::NAME,
                resource_type = allocation.resource_type(),
                resource_id = %allocation.resource.id,
                runtime_revision = allocation.resource.revision,
                stream_idle_timeout_seconds = timeout_seconds,
                "上游 SSE 达到连续无字节空闲超时，终止当前流"
            );
        } else {
            warn!(
                request_id = %self.request_id,
                provider = M::NAME,
                stream_idle_timeout_seconds = timeout_seconds,
                "上游 SSE 达到连续无字节空闲超时，终止当前流"
            );
        }

        // 与 reqwest 传输错误保持一致：先让 provider 完成残余协议解析，再丢弃不能构成
        // 完整 SSE 事件的输出。响应已经交给下游后不能安全重放，但公共网络波动仍不能
        // 归因到当前账号或 API Key，因此这里只终止流，不提交资源回执。
        self.complete_observer();
        self.output.clear();
        self.stream_error = Some(StreamErrorRecord {
            kind: "stream_idle_timeout",
            body: format!("上游 SSE 连续 {timeout_seconds} 秒未返回数据"),
        });
        self.pending_stream_error = Some(ManagedStreamError::IdleTimeout { timeout_seconds });
        self.stream_idle_sleep = None;
    }

    fn observe(&mut self, bytes: Bytes) {
        if let Some(observer) = self.observer.as_mut() {
            let update = observer.observe(bytes);
            self.output.extend(update.output);
            self.submit_feedback_once(update.feedback);
            return;
        }
        if self.plugin_session.is_none() {
            self.fail_plugin("stream 响应既没有 provider observer，也没有插件 session".to_owned());
            return;
        }
        self.plugin_sse_buffer.extend_from_slice(&bytes);
        let items = plugin::sse::drain_complete_items(&mut self.plugin_sse_buffer);
        if self.plugin_sse_buffer.len() > MAX_SSE_ITEM_BYTES {
            self.fail_plugin(format!(
                "原始 SSE item 超过缓冲上限: {} bytes（最大 {} bytes）",
                self.plugin_sse_buffer.len(),
                MAX_SSE_ITEM_BYTES
            ));
            return;
        }
        if items.is_empty() {
            return;
        }
        self.start_plugin_items(items);
    }

    fn start_plugin_items(&mut self, items: Vec<Bytes>) {
        let Some(session) = self.plugin_session.take() else {
            self.fail_plugin("stream 插件 session 在 item 调用前丢失".to_owned());
            return;
        };
        self.pending_plugin_work = Some(Box::pin(async move {
            plugin::transform_stream_items(session, items)
                .await
                .map(PendingPluginResult::Items)
        }));
    }

    fn start_plugin_finish(&mut self) {
        let Some(session) = self.plugin_session.take() else {
            self.fail_plugin("stream 插件 session 在 finish 前丢失".to_owned());
            return;
        };
        self.pending_plugin_work = Some(Box::pin(async move {
            plugin::finish_stream(session)
                .await
                .map(PendingPluginResult::Finish)
        }));
    }

    fn accept_plugin_items(&mut self, outputs: Vec<StreamPluginItemOutput>) -> AppResult<()> {
        for output in outputs {
            if let Some(item) = output.item {
                self.output.push_back(item);
            }
            self.accept_plugin_effects(output.effects, Some(&output.upstream_item))?;
        }
        Ok(())
    }

    fn accept_plugin_finish(&mut self, output: StreamPluginFinishOutput) -> AppResult<()> {
        self.output.extend(output.items);
        self.accept_plugin_effects(output.effects, None)
    }

    fn accept_plugin_effects(
        &mut self,
        effects: PluginEffects,
        upstream_item: Option<&Bytes>,
    ) -> AppResult<()> {
        if let Some(usage) = effects.usage {
            ensure_monotonic_usage(self.usage, usage)?;
            self.usage = Some(usage);
        }
        if let Some(failure) = effects.failure {
            let tracing_body = upstream_item.map(|item| response_body_for_tracing(item));
            warn!(
                request_id = %self.request_id,
                provider = M::NAME,
                plugin_failure_kind = %failure.kind,
                plugin_failure_message = %failure.message,
                upstream_response_body_bytes = upstream_item.map_or(0, |item| item.len()),
                upstream_response_body_encoding = tracing_body.as_ref().map(|body| body.encoding()).unwrap_or("unavailable"),
                upstream_response_body = tracing_body.as_ref().map(|body| body.content()).unwrap_or("<finish>"),
                "stream 响应插件报告上游协议失败事实"
            );
            self.stream_error = Some(StreamErrorRecord {
                kind: "plugin_sse_event",
                body: format!("{}: {}", failure.kind, failure.message),
            });
        }
        // output 已先进入队列；start_feedback_submission 不再移动字节，而 poll 顺序保证
        // pending feedback 完成前这些 item 不会出队。
        if effects.feedback.is_some() && self.feedback_submitted {
            return Err(AppError::Plugin {
                message: "stream 插件在同一响应中重复返回 feedback".to_owned(),
            });
        }
        self.submit_feedback_once(effects.feedback);
        Ok(())
    }

    fn fail_plugin(&mut self, message: String) {
        error!(request_id = %self.request_id, provider = M::NAME, error = %message, "stream 响应插件状态机失败");
        self.output.clear();
        self.plugin_sse_buffer.clear();
        self.plugin_session = None;
        self.stream_error = Some(StreamErrorRecord {
            kind: "plugin_error",
            body: message.clone(),
        });
        self.pending_stream_error = Some(ManagedStreamError::Plugin(message));
        self.stream_idle_sleep = None;
    }

    /// 在首个非空下游 chunk 产出前发布一次客户端视角的响应开始事实。
    ///
    /// observer 可能为了拼接完整 SSE event 暂存多个上游 chunk，也可能先等待资源回执；
    /// 因此必须在 `output` 真正出队时记录，不能使用上游响应头或原始字节到达时间代替。
    fn mark_response_started_once(&mut self, bytes: &Bytes) {
        if self.response_started_emitted || bytes.is_empty() {
            return;
        }
        self.response_started_emitted = true;
        self.state
            .request_events()
            .emit(RequestEvent::ResponseStarted {
                request_id: self.request_id,
                occurred_at: Utc::now(),
            });
    }

    fn submit_feedback_once(&mut self, feedback: Option<UpstreamFeedback>) {
        if self.feedback_submitted {
            return;
        }
        let Some(feedback) = feedback else {
            return;
        };
        self.feedback_submitted = true;
        self.start_feedback_submission(feedback);
    }

    fn start_feedback_submission(&mut self, feedback: UpstreamFeedback) {
        let Some(allocation) = self.lease.as_ref().map(|lease| lease.allocation().clone()) else {
            return;
        };
        let state = self.state.clone();
        let feedback_kind = feedback.as_str();
        let task = tokio::spawn(async move {
            let result = maintenance::apply_upstream_feedback::<M>(
                &state,
                allocation.request_id,
                &allocation.resource,
                feedback,
            )
            .await;
            match result {
                Ok(applied) => info!(
                    request_id = %allocation.request_id,
                    provider = M::NAME,
                    resource_type = allocation.resource_type(),
                    resource_id = %allocation.resource.id,
                    feedback = feedback_kind,
                    resource_feedback_applied = applied,
                    "流式上游事实的持久状态迁移与 runtime 隔离处理已完成"
                ),
                Err(error) => error!(
                    request_id = %allocation.request_id,
                    provider = M::NAME,
                    resource_type = allocation.resource_type(),
                    resource_id = %allocation.resource.id,
                    feedback = feedback_kind,
                    error = %error,
                    "流式上游事实的持久状态迁移或 runtime 隔离失败"
                ),
            }
        });
        self.pending_feedback_submission = Some(Box::pin(async move {
            if let Err(error) = task.await {
                error!(provider = M::NAME, error = %error, "流式资源回执处理任务异常结束");
            }
        }));
    }

    fn complete_observer(&mut self) {
        let Some(mut observer) = self.observer.take() else {
            return;
        };
        let completion = observer.complete();
        self.output.extend(completion.output);
        self.usage = completion.usage;
        self.stream_error = completion.error;
        self.submit_feedback_once(completion.feedback);
    }

    fn finish_once(&mut self, reason: StreamEndReason) {
        self.complete_observer();
        if self.plugin_upstream_eof
            && self.pending_plugin_work.is_none()
            && self.plugin_session.is_some()
        {
            self.start_plugin_finish();
        }
        let Some(lease) = self.lease.take() else {
            return;
        };
        let terminal_facts = StreamTerminalFacts {
            request_id: self.request_id,
            attribution: self.usage_attribution,
            response_finished_at: Utc::now(),
            reason,
            usage: self.usage.take(),
            error: self.stream_error.take(),
        };
        let pending_feedback_submission = self.pending_feedback_submission.take();
        let pending_plugin_work = self.pending_plugin_work.take();
        let feedback_already_submitted = self.feedback_submitted;
        let state = self.state.clone();

        let Some(pending_plugin_work) = pending_plugin_work else {
            // 原生 observer 以及已经正常完成 item/finish 的插件流，在这里已经拥有完整的
            // usage、错误与结束原因。事件发布本身是 try_send，不应因为 maintenance 或
            // lease 清理被推迟到后台任务，更不能额外承担后台任务取消带来的丢失窗口。
            emit_stream_terminal_events(&state, terminal_facts);
            spawn_stream_resource_cleanup::<M>(
                state,
                lease,
                pending_feedback_submission,
                None,
                reason,
            );
            return;
        };

        // 只有下游断开时仍有 item/finish blocking 任务在运行，最终 usage、failure 或
        // feedback 才尚未确定。Drop 不能 await，因此把这一小段事实补全移交后台；事实
        // 一旦确定就立即投递，后续 maintenance 与 lease release 仍属于独立资源清理层。
        tokio::spawn(async move {
            let (terminal_facts, cleanup_feedback) = resolve_pending_plugin_facts::<M>(
                terminal_facts,
                pending_plugin_work,
                feedback_already_submitted,
            )
            .await;
            emit_stream_terminal_events(&state, terminal_facts);
            finish_stream_resource_cleanup::<M>(
                state,
                lease,
                pending_feedback_submission,
                cleanup_feedback,
                reason,
            )
            .await;
        });
    }
}

/// 已经能够确定的流式请求事实。它与 maintenance future、插件 session 和 Redis lease
/// 完全分离，确保日志与额度事件只依赖协议观察结果，不依赖资源清理是否成功。
struct StreamTerminalFacts {
    request_id: uuid::Uuid,
    attribution: UsageAttribution,
    response_finished_at: chrono::DateTime<Utc>,
    reason: StreamEndReason,
    usage: Option<TokenUsage>,
    error: Option<StreamErrorRecord>,
}

/// 终态事件必须保持 usage 在前、Ended 在后。日志 worker 收到 Ended 后会移除聚合上下文，
/// 因此不能把可能补充日志字段的 usage 放到终态之后；两个 emit 都是非阻塞 try_send。
fn emit_stream_terminal_events(state: &AppState, facts: StreamTerminalFacts) {
    if let Some(usage) = facts.usage {
        state.request_events().emit(RequestEvent::UsageObserved {
            request_id: facts.request_id,
            attribution: facts.attribution,
            usage,
        });
    }
    state.request_events().emit(RequestEvent::Ended {
        request_id: facts.request_id,
        occurred_at: facts.response_finished_at,
        result: RequestEndResult::Stream {
            reason: facts.reason,
            error: facts.error,
        },
    });
}

/// 下游断开可能发生在同步 Wasmtime 调用所在的 blocking 任务尚未返回时。这里只等待并
/// 合并插件仍可能产生的最终事实；插件输出 item 已无下游消费者，故只保留 effects。
async fn resolve_pending_plugin_facts<M: MaintenanceProvider>(
    mut facts: StreamTerminalFacts,
    pending_plugin_work: PendingPluginWork,
    feedback_already_submitted: bool,
) -> (StreamTerminalFacts, Option<UpstreamFeedback>) {
    let mut cleanup_feedback = None;
    match pending_plugin_work.await {
        Ok(result) => {
            let effects = match result {
                PendingPluginResult::Items(batch) => {
                    if let Some(error) = batch.error {
                        facts.error = Some(StreamErrorRecord {
                            kind: "plugin_error",
                            body: error.to_string(),
                        });
                    }
                    batch
                        .outputs
                        .into_iter()
                        .map(|output| (output.effects, Some(output.upstream_item)))
                        .collect::<Vec<_>>()
                }
                PendingPluginResult::Finish(output) => vec![(output.effects, None)],
            };
            for (effects, upstream_item) in effects {
                if let Some(snapshot) = effects.usage {
                    match ensure_monotonic_usage(facts.usage, snapshot) {
                        Ok(()) => facts.usage = Some(snapshot),
                        Err(error) => {
                            facts.error = Some(StreamErrorRecord {
                                kind: "plugin_error",
                                body: error.to_string(),
                            });
                        }
                    }
                }
                if let Some(failure) = effects.failure {
                    let tracing_body = upstream_item
                        .as_ref()
                        .map(|item| response_body_for_tracing(item));
                    warn!(
                        request_id = %facts.request_id,
                        provider = M::NAME,
                        plugin_failure_kind = %failure.kind,
                        plugin_failure_message = %failure.message,
                        upstream_response_body_bytes = upstream_item.as_ref().map_or(0, Bytes::len),
                        upstream_response_body_encoding = tracing_body.as_ref().map(|body| body.encoding()).unwrap_or("unavailable"),
                        upstream_response_body = tracing_body.as_ref().map(|body| body.content()).unwrap_or("<finish>"),
                        "下游断开后的 stream 插件协议失败事实已完成记录"
                    );
                    facts.error = Some(StreamErrorRecord {
                        kind: "plugin_sse_event",
                        body: format!("{}: {}", failure.kind, failure.message),
                    });
                }
                if let Some(feedback) = effects.feedback {
                    if feedback_already_submitted || cleanup_feedback.is_some() {
                        facts.error = Some(StreamErrorRecord {
                            kind: "plugin_error",
                            body: "stream 插件在同一响应中重复返回 feedback".to_owned(),
                        });
                    } else {
                        cleanup_feedback = Some(feedback);
                    }
                }
            }
        }
        Err(error) => {
            facts.error = Some(StreamErrorRecord {
                kind: "plugin_error",
                body: error.to_string(),
            });
        }
    }
    (facts, cleanup_feedback)
}

/// 已确定请求终态后的资源清理。已有 feedback 与插件收尾 feedback 都必须先于 release，
/// 确保故障资源的持久状态和 Redis 隔离完成后再减少 inflight；清理失败只写日志，不反向
/// 修改已经发生并投递的请求事实。
fn spawn_stream_resource_cleanup<M: MaintenanceProvider>(
    state: AppState,
    lease: UpstreamLease,
    pending_feedback_submission: Option<PendingFeedbackSubmission>,
    cleanup_feedback: Option<UpstreamFeedback>,
    reason: StreamEndReason,
) {
    tokio::spawn(finish_stream_resource_cleanup::<M>(
        state,
        lease,
        pending_feedback_submission,
        cleanup_feedback,
        reason,
    ));
}

async fn finish_stream_resource_cleanup<M: MaintenanceProvider>(
    state: AppState,
    lease: UpstreamLease,
    pending_feedback_submission: Option<PendingFeedbackSubmission>,
    cleanup_feedback: Option<UpstreamFeedback>,
    reason: StreamEndReason,
) {
    let allocation = lease.allocation().clone();
    if let Some(submission) = pending_feedback_submission {
        submission.await;
    }
    if let Some(feedback) = cleanup_feedback
        && let Err(error) = maintenance::apply_upstream_feedback::<M>(
            &state,
            allocation.request_id,
            &allocation.resource,
            feedback,
        )
        .await
    {
        error!(request_id = %allocation.request_id, provider = M::NAME, error = %error, "下游断开后的插件 feedback 收尾失败");
    }
    if let Err(error) = lease.release().await {
        error!(
            request_id = %allocation.request_id,
            provider = M::NAME,
            resource_type = allocation.resource_type(),
            resource_id = %allocation.resource.id,
            reason = reason.as_str(),
            error = %error,
            "流式响应结束后释放上游资源失败"
        );
    }
}

impl<M: MaintenanceProvider> Stream for ManagedUpstreamStream<M> {
    type Item = Result<Bytes, ManagedStreamError>;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            if let Some(future) = self.pending_feedback_submission.as_mut() {
                match future.as_mut().poll(context) {
                    Poll::Ready(()) => {
                        self.pending_feedback_submission = None;
                        continue;
                    }
                    Poll::Pending => return Poll::Pending,
                }
            }
            if let Some(future) = self.pending_plugin_work.as_mut() {
                match future.as_mut().poll(context) {
                    Poll::Ready(Ok(PendingPluginResult::Items(batch))) => {
                        self.pending_plugin_work = None;
                        self.plugin_session = Some(batch.session);
                        if let Err(error) = self.accept_plugin_items(batch.outputs) {
                            self.fail_plugin(error.to_string());
                        } else if let Some(error) = batch.error {
                            self.fail_plugin(error.to_string());
                        }
                        continue;
                    }
                    Poll::Ready(Ok(PendingPluginResult::Finish(output))) => {
                        self.pending_plugin_work = None;
                        if let Err(error) = self.accept_plugin_finish(output) {
                            self.fail_plugin(error.to_string());
                        } else {
                            self.finish_after_output = Some(StreamEndReason::UpstreamEof);
                        }
                        continue;
                    }
                    Poll::Ready(Err(error)) => {
                        self.pending_plugin_work = None;
                        self.fail_plugin(error.to_string());
                        continue;
                    }
                    Poll::Pending => return Poll::Pending,
                }
            }
            if let Some(bytes) = self.output.pop_front() {
                self.mark_response_started_once(&bytes);
                return Poll::Ready(Some(Ok(bytes)));
            }
            if let Some(error) = self.pending_stream_error.take() {
                let reason = match &error {
                    ManagedStreamError::Upstream(_) => StreamEndReason::UpstreamError,
                    ManagedStreamError::IdleTimeout { .. } => StreamEndReason::IdleTimeout,
                    ManagedStreamError::Plugin(_) => StreamEndReason::PluginError,
                };
                self.finish_once(reason);
                return Poll::Ready(Some(Err(error)));
            }
            if let Some(reason) = self.finish_after_output.take() {
                self.finish_once(reason);
                return Poll::Ready(None);
            }
            if self.plugin_upstream_eof {
                if !self.plugin_sse_buffer.is_empty() {
                    let buffered_bytes = self.plugin_sse_buffer.len();
                    self.fail_plugin(format!(
                        "上游 EOF 时存在未完成 SSE item: {buffered_bytes} bytes"
                    ));
                    continue;
                }
                if self.plugin_session.is_some() {
                    self.start_plugin_finish();
                    continue;
                }
            }

            match self.stream.as_mut().poll_next(context) {
                Poll::Ready(Some(Ok(bytes))) => {
                    self.reset_stream_idle_deadline();
                    self.observe(bytes);
                }
                Poll::Ready(Some(Err(error))) => {
                    if let Some(allocation) = self.lease.as_ref().map(UpstreamLease::allocation) {
                        warn!(
                            request_id = %self.request_id,
                            provider = M::NAME,
                            resource_type = allocation.resource_type(),
                            resource_id = %allocation.resource.id,
                            runtime_revision = allocation.resource.revision,
                            error = %error,
                            "读取上游 SSE 字节流失败；响应已交给下游，终止当前流且不提交资源回执"
                        );
                    } else {
                        warn!(
                            request_id = %self.request_id,
                            provider = M::NAME,
                            error = %error,
                            "读取上游 SSE 字节流失败；响应已交给下游，终止当前流且不提交资源回执"
                        );
                    }
                    self.complete_observer();
                    self.output.clear();
                    self.stream_error = Some(StreamErrorRecord::fluctuation());
                    self.pending_stream_error = Some(ManagedStreamError::Upstream(error));
                    self.stream_idle_sleep = None;
                }
                Poll::Ready(None) => {
                    self.stream_idle_sleep = None;
                    if self.observer.is_some() {
                        self.complete_observer();
                        self.finish_after_output = Some(StreamEndReason::UpstreamEof);
                    } else {
                        self.plugin_upstream_eof = true;
                    }
                }
                Poll::Pending => {
                    // 先 poll 上游，再检查 timer。两者恰好同时 ready 时优先接收字节并重置
                    // deadline，避免在超时边界把已经到达的数据误判为空闲。
                    let idle_timeout_reached = self
                        .stream_idle_sleep
                        .as_mut()
                        .is_some_and(|sleep| sleep.as_mut().poll(context).is_ready());
                    if idle_timeout_reached {
                        self.handle_stream_idle_timeout();
                        continue;
                    }
                    return Poll::Pending;
                }
            }
        }
    }
}

fn ensure_monotonic_usage(previous: Option<TokenUsage>, current: TokenUsage) -> AppResult<()> {
    let Some(previous) = previous else {
        return Ok(());
    };
    let monotonic = current.input_tokens >= previous.input_tokens
        && current.cached_input_tokens >= previous.cached_input_tokens
        && current.output_tokens >= previous.output_tokens
        && current.reasoning_output_tokens >= previous.reasoning_output_tokens
        && current.total_tokens >= previous.total_tokens;
    if monotonic {
        return Ok(());
    }
    Err(AppError::Plugin {
        message: format!(
            "stream 插件 usage snapshot 发生回退: previous={}/{}/{}/{}/{}, current={}/{}/{}/{}/{}",
            previous.input_tokens,
            previous.cached_input_tokens,
            previous.output_tokens,
            previous.reasoning_output_tokens,
            previous.total_tokens,
            current.input_tokens,
            current.cached_input_tokens,
            current.output_tokens,
            current.reasoning_output_tokens,
            current.total_tokens,
        ),
    })
}

impl<M: MaintenanceProvider> Drop for ManagedUpstreamStream<M> {
    fn drop(&mut self) {
        let Some(allocation) = self.lease.as_ref().map(UpstreamLease::allocation) else {
            return;
        };
        warn!(
            request_id = %self.request_id,
            provider = M::NAME,
            resource_type = allocation.resource_type(),
            resource_id = %allocation.resource.id,
            runtime_revision = allocation.resource.revision,
            pending_output_chunks = self.output.len(),
            feedback_pending = self.pending_feedback_submission.is_some(),
            plugin_pending = self.pending_plugin_work.is_some(),
            "下游在流式响应 EOF 前停止消费，记录 downstream_disconnected 并释放上游资源"
        );
        // 下游取消不代表资源故障，因此不新增 feedback；已有 provider 错误回执仍移交
        // 后台等待完成，然后按与其他结束路径相同的顺序释放 inflight lease。
        self.finish_once(StreamEndReason::DownstreamDisconnected);
    }
}
