use std::future::Future;

use axum::{
    body::Bytes,
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode},
};
use tracing::{debug, info, warn};

use crate::{
    config::AppConfig,
    err::{AppError, AppResult},
    infra::http_client::HttpClientProfile,
    provider::{
        gpt::{
            codex_http::{
                header as codex_header, request as codex_request,
                response::{
                    self as codex_response, CodexAccountSignal, CodexSseData, CodexTokenUsage,
                },
            },
            maintenance::GptMaintenance,
            model::GptAccountRequestContext,
        },
        protocol::{
            BufferedProtocolResponse, EncodedProviderError, MAX_SSE_ITEM_BYTES, ProtocolFailure,
            ProtocolResponse, ProviderProtocol, ProviderVisibleError, ReplayableRequest,
            RequestInspection, RequestLogFields, StreamCompletion, StreamErrorRecord,
            StreamObserver, StreamUpdate, StreamingProtocolResponse, TokenUsage,
            UpstreamAttemptContext, UpstreamFeedback, UpstreamRequestBodyMode,
            UpstreamRequestDraft, UpstreamRequestTarget, read_buffered_upstream_body,
        },
        resource::{UpstreamResource, UpstreamResourceKind},
        response_logging::response_body_for_tracing,
    },
};

/// ChatGPT 账号 Responses 上游返回的账号级 Codex 窗口额度 header。
///
/// 网关调用方使用的是网关 API Key，不应观察到某一次调度所选 OAuth 账号的剩余额度；
/// 否则 Codex API Key 模式会把这个短暂账号快照缓存并展示在 `/status` 中，既泄露池内账号
/// 状态，也会让后续切换账号后的额度展示产生误导。这里只过滤 Codex 当前消费的两个默认
/// 窗口字段，其他响应 header 继续沿用既有透明代理规则。
const ACCOUNT_CODEX_RATE_LIMIT_RESPONSE_HEADERS: [&str; 6] = [
    "x-codex-primary-used-percent",
    "x-codex-primary-window-minutes",
    "x-codex-primary-reset-at",
    "x-codex-secondary-used-percent",
    "x-codex-secondary-window-minutes",
    "x-codex-secondary-reset-at",
];

impl From<CodexTokenUsage> for TokenUsage {
    fn from(usage: CodexTokenUsage) -> Self {
        Self {
            input_tokens: usage.input_tokens,
            cached_input_tokens: usage.cached_input_tokens,
            output_tokens: usage.output_tokens,
            reasoning_output_tokens: usage.reasoning_output_tokens,
            total_tokens: usage.total_tokens,
        }
    }
}

/// GPT Responses 的 provider/operation adapter。
///
/// HTTP 入口、鉴权、请求体缓存、调度、重试与生命周期收尾均由通用 pipeline 负责；
/// 这里仅保留 Codex 元数据、认证、错误分类及 SSE 协议差异。
pub struct GptResponsesProxy;

impl ProviderProtocol for GptResponsesProxy {
    type Maintenance = GptMaintenance;

    fn inspect_request(body: &[u8]) -> AppResult<RequestInspection> {
        let metadata = codex_request::parse_responses_metadata(body)?;
        let sticky_key = metadata
            .prompt_cache_key
            .as_ref()
            .map(|value| format!("responses:prompt_cache_key:{value}"));
        let reasoning = metadata
            .reasoning
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|source| AppError::BadRequest {
                message: format!("请求体 reasoning 字段无法序列化为 JSON: {source}"),
            })?;
        Ok(RequestInspection {
            requested_model: metadata.model,
            sticky_key,
            log_fields: RequestLogFields {
                reasoning,
                service_tier: metadata.service_tier,
                fast_mode: None,
                is_compaction: Some(metadata.is_compaction),
            },
        })
    }

    fn encode_error(error: &ProviderVisibleError, request_id: uuid::Uuid) -> EncodedProviderError {
        codex_response::encode_provider_error(error, request_id)
    }

    fn prepare_upstream_target(
        config: &AppConfig,
        resource: &UpstreamResource,
        request: &ReplayableRequest,
    ) -> AppResult<UpstreamRequestTarget> {
        let (url, client_profile) = match resource.kind {
            UpstreamResourceKind::Account => (
                build_upstream_url(
                    config.gpt_upstream_base_url.trim_end_matches('/'),
                    &config.gpt_upstream_responses_path,
                    request.uri.query(),
                ),
                HttpClientProfile::ChatGptCodex,
            ),
            UpstreamResourceKind::ApiKey => (
                build_official_url(
                    resource.api_key_base_url()?,
                    &config.gpt_upstream_responses_path,
                    request.uri.query(),
                ),
                HttpClientProfile::Generic,
            ),
        };
        Ok(UpstreamRequestTarget {
            client_profile,
            method: reqwest::Method::POST,
            url,
        })
    }

    fn prepare_upstream_request(
        config: &AppConfig,
        resource: &UpstreamResource,
        request: &ReplayableRequest,
    ) -> AppResult<UpstreamRequestDraft> {
        let target = Self::prepare_upstream_target(config, resource, request)?;
        let headers = match resource.kind {
            UpstreamResourceKind::Account => codex_header::build_codex_upstream_headers(
                &request.headers,
                config.codex_version_header.as_deref(),
            ),
            UpstreamResourceKind::ApiKey => {
                codex_header::build_official_api_key_upstream_headers(&request.headers)
            }
        };

        info!(
            resource_type = resource.kind.as_str(),
            resource_id = %resource.id,
            upstream_url = %target.url,
            query_passthrough = request.uri.query().is_some(),
            body_bytes = request.body.len(),
            "GPT Responses adapter 已生成上游请求草稿"
        );

        Ok(UpstreamRequestDraft {
            client_profile: target.client_profile,
            method: target.method,
            url: target.url,
            headers,
            body: UpstreamRequestBodyMode::ReplayOriginal,
        })
    }

    fn finalize_upstream_request(
        resource: &UpstreamResource,
        _request_id: uuid::Uuid,
        headers: &mut HeaderMap,
        _body: Option<&mut Bytes>,
    ) -> AppResult<()> {
        // GPT 当前只需要最终化认证 header；可选 body 已由通用 pipeline 完成 override，
        // 没有 body override 时仍直接从请求缓存重放，不因凭证注入而物化完整请求。
        match resource.kind {
            UpstreamResourceKind::Account => {
                let context = resource.parse_request_context::<GptAccountRequestContext>()?;
                codex_header::apply_codex_credential(
                    headers,
                    codex_header::CodexAccountAuth {
                        access_token: resource.auth_secret.trim(),
                        chatgpt_account_id: context.chatgpt_account_id.as_deref(),
                        chatgpt_account_is_fedramp: context.chatgpt_account_is_fedramp,
                    },
                )?;
            }
            UpstreamResourceKind::ApiKey => {
                codex_header::apply_official_api_key_credential(
                    headers,
                    resource.auth_secret.trim(),
                )?;
            }
        }
        Ok(())
    }

    fn handle_response<'a>(
        config: &'a AppConfig,
        resource: &'a UpstreamResource,
        attempt: UpstreamAttemptContext,
        response: reqwest::Response,
    ) -> impl Future<Output = Result<ProtocolResponse, ProtocolFailure>> + Send + 'a {
        process_upstream_response(config, resource, attempt, response)
    }
}

fn account_signal_to_feedback(signal: CodexAccountSignal) -> UpstreamFeedback {
    match signal {
        CodexAccountSignal::Unauthorized => UpstreamFeedback::AuthenticationRejected {
            reason: "unauthorized".to_owned(),
        },
        CodexAccountSignal::QuotaExhausted { resets_at } => UpstreamFeedback::QuotaExhausted {
            resets_at,
            reason: "quota_exhausted".to_owned(),
        },
        CodexAccountSignal::UsageLimitReached {
            plan_type,
            resets_at,
        } => UpstreamFeedback::QuotaExhausted {
            resets_at,
            reason: format!(
                "usage_limit_reached: plan_type={}",
                plan_type.as_deref().unwrap_or("<unknown>")
            ),
        },
        CodexAccountSignal::UsageNotIncluded => UpstreamFeedback::EntitlementMissing {
            reason: "usage_not_included".to_owned(),
        },
    }
}

async fn process_upstream_response(
    config: &AppConfig,
    resource: &UpstreamResource,
    attempt: UpstreamAttemptContext,
    upstream_response: reqwest::Response,
) -> Result<ProtocolResponse, ProtocolFailure> {
    let status = upstream_response.status();
    let headers = upstream_response.headers().clone();

    if status.is_success() {
        info!(
            resource_type = resource.kind.as_str(),
            resource_id = %resource.id,
            upstream_status = status.as_u16(),
            "GPT 上游返回成功响应，开始按字节透传并旁路观察 SSE 协议"
        );
        return Ok(ProtocolResponse::Streaming(StreamingProtocolResponse {
            status,
            headers: filtered_response_headers(&headers, resource.kind),
            stream: Box::pin(upstream_response.bytes_stream()),
            observer: Some(Box::new(GptSseObserver::new(resource.kind))),
        }));
    }

    let body = read_buffered_upstream_body(config, attempt.provider, upstream_response).await?;
    let tracing_body = response_body_for_tracing(&body);
    warn!(
        request_id = %attempt.request_id,
        provider = attempt.provider,
        resource_type = attempt.resource_kind.as_str(),
        resource_id = %attempt.resource_id,
        runtime_revision = attempt.runtime_revision,
        attempt_number = attempt.attempt_number,
        max_attempts = attempt.max_attempts,
        upstream_status = status.as_u16(),
        response_bytes = body.len(),
        upstream_response_body_encoding = tracing_body.encoding(),
        upstream_response_body = %tracing_body.content(),
        "GPT 上游返回失败响应，完整响应正文已写入 tracing"
    );

    let account_signal = codex_response::parse_account_signal(status, &body);
    let response_headers = filtered_response_headers(&headers, resource.kind);
    let (retry, exclude_resource_on_retry, feedback) =
        if resource.kind == UpstreamResourceKind::ApiKey {
            let decision = classify_api_key_http_failure(status);
            debug!(
                upstream_status = status.as_u16(),
                retry = decision.retry,
                exclude_resource_on_retry = decision.exclude_resource_on_retry,
                quarantine_key = decision.quarantine_key,
                "GPT 官方 API Key HTTP 错误已完成请求级/资源级分类"
            );
            let feedback = decision.quarantine_key.then(|| UpstreamFeedback::Error {
                reason: format!("HTTP {status}"),
            });
            (decision.retry, decision.exclude_resource_on_retry, feedback)
        } else if let Some(signal) = account_signal {
            // usage_not_included 只说明当前账号不能承载这次调用，不足以改变账号对其他请求的
            // 持久健康状态；但本请求重试时也不能再次选回同一账号。
            let exclude_resource_on_retry = matches!(&signal, CodexAccountSignal::UsageNotIncluded);
            (
                true,
                exclude_resource_on_retry,
                Some(account_signal_to_feedback(signal)),
            )
        } else {
            (is_transient_upstream_status(status), false, None)
        };

    Ok(ProtocolResponse::Buffered(BufferedProtocolResponse {
        status,
        headers: response_headers,
        body,
        record_error_response: true,
        retry,
        exclude_resource_on_retry,
        feedback,
        usage: None,
    }))
}

/// GPT 官方 API Key 的 HTTP 失败分类。
///
/// `retry`、`exclude_resource_on_retry` 和 `quarantine_key` 分别代表三种不同
/// 的生命周期语义：是否允许当前请求换资源重试、是否只在当前请求的排除集合中
/// 暂时排除该 Key，以及是否通过 `UpstreamFeedback` 把 Key 的错误写入全局维护
/// 状态。408 和 5xx 属于上游/网络的瞬态失败，不能据此认定 Key 本身失效，因而
/// 仅执行前两项；401、403、429 仍保留原有的全局 Key 错误回执策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ApiKeyHttpFailureDecision {
    retry: bool,
    exclude_resource_on_retry: bool,
    quarantine_key: bool,
}

fn classify_api_key_http_failure(status: StatusCode) -> ApiKeyHttpFailureDecision {
    if matches!(
        status,
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN | StatusCode::TOO_MANY_REQUESTS
    ) {
        return ApiKeyHttpFailureDecision {
            retry: true,
            exclude_resource_on_retry: false,
            quarantine_key: true,
        };
    }

    if is_transient_upstream_status(status) {
        return ApiKeyHttpFailureDecision {
            retry: true,
            exclude_resource_on_retry: true,
            quarantine_key: false,
        };
    }

    ApiKeyHttpFailureDecision {
        retry: false,
        exclude_resource_on_retry: false,
        quarantine_key: false,
    }
}

fn is_transient_upstream_status(status: StatusCode) -> bool {
    status == StatusCode::REQUEST_TIMEOUT || status.is_server_error()
}

fn normalized_path(path: &str) -> String {
    if path.starts_with('/') {
        path.to_owned()
    } else {
        format!("/{path}")
    }
}

fn build_upstream_url(base_url: &str, path: &str, query: Option<&str>) -> String {
    let mut url = format!("{}{}", base_url, normalized_path(path));
    if let Some(query) = query.filter(|value| !value.is_empty()) {
        url.push('?');
        url.push_str(query);
    }
    url
}

fn build_official_url(base_url: &str, path: &str, query: Option<&str>) -> String {
    build_upstream_url(base_url.trim().trim_end_matches('/'), path, query)
}

fn filtered_response_headers(source: &HeaderMap, resource_kind: UpstreamResourceKind) -> HeaderMap {
    let mut headers = HeaderMap::new();
    copy_response_headers(source, &mut headers, resource_kind);

    let filtered_account_rate_limit_headers = if resource_kind == UpstreamResourceKind::Account {
        ACCOUNT_CODEX_RATE_LIMIT_RESPONSE_HEADERS
            .iter()
            .copied()
            .filter(|name| source.contains_key(*name))
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    debug!(
        resource_type = resource_kind.as_str(),
        upstream_header_value_count = source.len(),
        downstream_header_value_count = headers.len(),
        filtered_account_rate_limit_header_count = filtered_account_rate_limit_headers.len(),
        filtered_account_rate_limit_headers = ?filtered_account_rate_limit_headers,
        "GPT 原生响应头过滤完成"
    );
    headers
}

fn copy_response_headers(
    source: &HeaderMap,
    target: &mut HeaderMap,
    resource_kind: UpstreamResourceKind,
) {
    for (name, value) in source {
        if codex_response::should_forward_response_header(name)
            && should_forward_account_rate_limit_header(resource_kind, name)
            && let Ok(value) = HeaderValue::from_bytes(value.as_bytes())
        {
            target.append(name.clone(), value);
        }
    }
}

/// 额度 header 只属于上游 OAuth 账号，官方 API Key 仍保持透明响应语义。
///
/// 响应插件命中时通用 pipeline 会完全绕过 `ProviderProtocol::handle_response`，因此不会
/// 调用这里；插件生成的同名 header 仍由插件运行时按原有安全规则处理，不受本过滤影响。
fn should_forward_account_rate_limit_header(
    resource_kind: UpstreamResourceKind,
    name: &HeaderName,
) -> bool {
    resource_kind != UpstreamResourceKind::Account
        || !ACCOUNT_CODEX_RATE_LIMIT_RESPONSE_HEADERS.contains(&name.as_str())
}

/// GPT SSE observer 只保存协议解析状态；maintenance 回执、资源释放、额度扣减与日志收尾
/// 全部由通用流包装器执行。
struct GptSseObserver {
    resource_kind: UpstreamResourceKind,
    sse_buffer: Vec<u8>,
    usage: Option<TokenUsage>,
    error: Option<StreamErrorRecord>,
    feedback_emitted: bool,
}

impl GptSseObserver {
    fn new(resource_kind: UpstreamResourceKind) -> Self {
        Self {
            resource_kind,
            sse_buffer: Vec::with_capacity(8192),
            usage: None,
            error: None,
            feedback_emitted: false,
        }
    }

    fn process_buffered_events(&mut self) -> StreamUpdate {
        let mut update = StreamUpdate::default();
        while let Some(boundary) = codex_response::find_sse_event_boundary(&self.sse_buffer) {
            let event = self
                .sse_buffer
                .drain(..boundary.event_end)
                .collect::<Vec<_>>();
            let delimiter = self
                .sse_buffer
                .drain(..boundary.delimiter_len)
                .collect::<Vec<_>>();
            let (output, feedback) = self.inspect_event(&event, &delimiter);
            update.output.push_back(output);
            if update.feedback.is_none() {
                update.feedback = feedback;
            }
        }

        if self.sse_buffer.len() > MAX_SSE_ITEM_BYTES {
            warn!(
                buffered_bytes = self.sse_buffer.len(),
                max_sse_item_bytes = MAX_SSE_ITEM_BYTES,
                "GPT SSE 事件缓冲区超过上限，按原始字节透传当前缓冲内容"
            );
            update
                .output
                .push_back(Bytes::from(std::mem::take(&mut self.sse_buffer)));
        }
        update
    }

    fn inspect_event(
        &mut self,
        event: &[u8],
        delimiter: &[u8],
    ) -> (Bytes, Option<UpstreamFeedback>) {
        let original = codex_response::original_sse_event_bytes(event, delimiter);
        let Some(data) = codex_response::collect_sse_event_data(event) else {
            return (original, None);
        };

        match codex_response::parse_sse_data_json(data.as_bytes()) {
            Some(CodexSseData::ResponseCompleted(usage)) => {
                self.record_usage(usage);
                (original, None)
            }
            Some(CodexSseData::ResponseFailed(error)) => self.record_failure(error, original),
            Some(CodexSseData::Other(_)) | None => (original, None),
        }
    }

    fn record_usage(&mut self, usage: CodexTokenUsage) {
        if self.usage.is_some() {
            return;
        }
        info!(
            input_tokens = usage.input_tokens,
            cached_input_tokens = usage.cached_input_tokens,
            output_tokens = usage.output_tokens,
            reasoning_output_tokens = usage.reasoning_output_tokens,
            total_tokens = usage.total_tokens,
            "GPT SSE response.completed 已旁路提取 token 用量"
        );
        self.usage = Some(usage.into());
    }

    fn record_failure(
        &mut self,
        error: codex_response::CodexResponseError,
        original: Bytes,
    ) -> (Bytes, Option<UpstreamFeedback>) {
        let original_for_request_log = String::from_utf8_lossy(&original).to_string();
        let tracing_body = response_body_for_tracing(&original);
        let Some(signal) = codex_response::parse_stream_account_signal(&error) else {
            warn!(
                resource_type = self.resource_kind.as_str(),
                upstream_response_body_bytes = original.len(),
                upstream_response_body_encoding = tracing_body.encoding(),
                upstream_response_body = %tracing_body.content(),
                "GPT SSE response.failed 不包含资源维护信号，完整原始事件已写入 tracing 并保持透传"
            );
            self.error = Some(StreamErrorRecord {
                kind: "sse_event",
                body: original_for_request_log,
            });
            return (original, None);
        };

        let client_retry_event = codex_response::client_retry_failed_event();
        self.error = Some(StreamErrorRecord {
            ..StreamErrorRecord::fluctuation()
        });

        let feedback = if self.feedback_emitted {
            None
        } else {
            self.feedback_emitted = true;
            Some(match self.resource_kind {
                UpstreamResourceKind::Account => account_signal_to_feedback(signal),
                UpstreamResourceKind::ApiKey => api_key_stream_signal_to_feedback(signal),
            })
        };
        warn!(
            resource_type = self.resource_kind.as_str(),
            upstream_response_body_bytes = original.len(),
            upstream_response_body_encoding = tracing_body.encoding(),
            upstream_response_body = %tracing_body.content(),
            feedback = feedback.as_ref().map(UpstreamFeedback::as_str).unwrap_or("already_emitted"),
            "GPT SSE response.failed 完整原始事件已写入 tracing，并已转换为中立回执和固定 client retry 事件"
        );
        (client_retry_event, feedback)
    }
}

impl StreamObserver for GptSseObserver {
    fn observe(&mut self, chunk: Bytes) -> StreamUpdate {
        self.sse_buffer.extend_from_slice(&chunk);
        self.process_buffered_events()
    }

    fn complete(&mut self) -> StreamCompletion {
        let mut update = self.process_buffered_events();
        if !self.sse_buffer.is_empty() {
            update
                .output
                .push_back(Bytes::from(std::mem::take(&mut self.sse_buffer)));
        }
        StreamCompletion {
            output: update.output,
            feedback: update.feedback,
            usage: self.usage.take(),
            error: self.error.take(),
        }
    }
}

fn api_key_stream_signal_to_feedback(signal: CodexAccountSignal) -> UpstreamFeedback {
    let reason = match signal {
        CodexAccountSignal::Unauthorized => "stream_unauthorized",
        CodexAccountSignal::QuotaExhausted { resets_at }
        | CodexAccountSignal::UsageLimitReached { resets_at, .. } => {
            let _ = resets_at;
            "stream_quota_limited"
        }
        CodexAccountSignal::UsageNotIncluded => "stream_usage_not_included",
    };
    UpstreamFeedback::Error {
        reason: reason.to_owned(),
    }
}
