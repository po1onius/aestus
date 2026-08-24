use std::future::Future;

use axum::{body::Bytes, http::HeaderMap};
use tracing::{info, warn};

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
            upstream::{
                account_signal_to_feedback, build_upstream_url, classify_http_failure,
                filtered_response_headers,
            },
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

    fn inspect_request(
        _headers: &HeaderMap,
        body: Bytes,
    ) -> impl Future<Output = AppResult<RequestInspection>> + Send {
        let result = (|| {
            let metadata = codex_request::parse_responses_metadata(&body)?;
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
        })();
        std::future::ready(result)
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
                build_upstream_url(
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
    let classification = classify_http_failure(resource.kind, status, account_signal);

    Ok(ProtocolResponse::Buffered(BufferedProtocolResponse {
        status,
        headers: response_headers,
        body,
        record_error_response: true,
        retry: classification.retry,
        exclude_resource_on_retry: classification.exclude_resource_on_retry,
        feedback: classification.feedback,
        usage: None,
    }))
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
