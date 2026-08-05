use std::future::Future;

use axum::{
    body::Bytes,
    http::{HeaderMap, StatusCode, header},
};
use chrono::{DateTime, Utc};
use tracing::{debug, info, warn};

use crate::{
    config::AppConfig,
    err::{AppError, AppResult},
    infra::http_client::HttpClientProfile,
    provider::{
        claude::{
            maintenance::ClaudeMaintenance,
            messages_http::{
                build_upstream_url, request as claude_request, request_header as claude_header,
                response as claude_response,
            },
            model::{ClaudeAccountRequestContext, PROVIDER},
        },
        protocol::{
            BufferedProtocolResponse, EncodedProviderError, MAX_SSE_ITEM_BYTES, ProtocolFailure,
            ProtocolResponse, ProviderProtocol, ProviderVisibleError, ReplayableRequest,
            RequestInspection, RequestLogFields, StreamCompletion, StreamErrorRecord,
            StreamObserver, StreamUpdate, StreamingProtocolResponse, UpstreamAttemptContext,
            UpstreamFeedback, UpstreamRequestBodyMode, UpstreamRequestDraft, UpstreamRequestTarget,
            read_buffered_upstream_body,
        },
        resource::{UpstreamResource, UpstreamResourceKind},
        response_logging::response_body_for_tracing,
    },
};

const COUNT_TOKENS_PATH: &str = "/v1/messages/count_tokens";

/// Claude 原生 Messages API adapter。
///
/// `/messages` 与 `/messages/count_tokens` 共用完整 pipeline；本模块只根据 URI 选择上游
/// operation，并实现 Anthropic 请求头、OAuth 认证、响应分类及 SSE 旁路观察。
pub struct ClaudeMessagesProxy;

impl ProviderProtocol for ClaudeMessagesProxy {
    type Maintenance = ClaudeMaintenance;

    fn inspect_request(body: &[u8]) -> AppResult<RequestInspection> {
        let metadata = claude_request::parse_messages_metadata(body)?;
        let sticky_key = metadata
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.user_id.clone())
            .map(|value| format!("messages:metadata.user_id:{value}"))
            .or_else(|| {
                metadata
                    .container_sticky_value()
                    .map(|value| format!("messages:container:{value}"))
            });
        let reasoning = metadata
            .thinking
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|source| AppError::BadRequest {
                message: format!("Claude Messages thinking 字段无法序列化: {source}"),
            })?;
        let fast_mode = match metadata.speed.as_deref() {
            Some("fast") => Some(true),
            Some("standard") => Some(false),
            _ => None,
        };
        Ok(RequestInspection {
            requested_model: metadata.model,
            sticky_key,
            log_fields: RequestLogFields {
                reasoning,
                service_tier: metadata.service_tier,
                fast_mode,
                is_compaction: None,
            },
        })
    }

    fn encode_error(error: &ProviderVisibleError, request_id: uuid::Uuid) -> EncodedProviderError {
        claude_response::encode_provider_error(error, request_id)
    }

    fn prepare_upstream_target(
        config: &AppConfig,
        resource: &UpstreamResource,
        request: &ReplayableRequest,
    ) -> AppResult<UpstreamRequestTarget> {
        let upstream_path = if request.uri.path() == COUNT_TOKENS_PATH {
            format!(
                "{}/count_tokens",
                config.claude_upstream_messages_path.trim_end_matches('/')
            )
        } else {
            config.claude_upstream_messages_path.clone()
        };
        let base_url = match resource.kind {
            UpstreamResourceKind::Account => config.claude_upstream_base_url.as_str(),
            UpstreamResourceKind::ApiKey => resource.api_key_base_url()?,
        };
        Ok(UpstreamRequestTarget {
            client_profile: HttpClientProfile::Generic,
            method: reqwest::Method::POST,
            url: build_upstream_url(base_url, &upstream_path, request.uri.query()),
        })
    }

    fn prepare_upstream_request(
        config: &AppConfig,
        resource: &UpstreamResource,
        request: &ReplayableRequest,
    ) -> AppResult<UpstreamRequestDraft> {
        let target = Self::prepare_upstream_target(config, resource, request)?;
        let headers = match resource.kind {
            UpstreamResourceKind::Account => {
                claude_header::build_account_upstream_headers(&request.headers)
            }
            UpstreamResourceKind::ApiKey => {
                claude_header::build_official_api_key_upstream_headers(&request.headers)
            }
        };

        info!(
            resource_type = resource.kind.as_str(),
            resource_id = %resource.id,
            operation = if request.uri.path() == COUNT_TOKENS_PATH { "count_tokens" } else { "messages" },
            upstream_url = %target.url,
            query_passthrough = request.uri.query().is_some(),
            body_bytes = request.body.len(),
            "Claude adapter 已生成上游请求草稿"
        );

        Ok(UpstreamRequestDraft {
            client_profile: target.client_profile,
            method: target.method,
            url: target.url,
            headers,
            // 只有 OAuth 账号需要按实际分配身份重写 metadata；官方 API Key 在没有
            // body override 时继续从不可变缓存直接重放原始请求。
            body: match resource.kind {
                UpstreamResourceKind::Account => UpstreamRequestBodyMode::MaterializeOriginal,
                UpstreamResourceKind::ApiKey => UpstreamRequestBodyMode::ReplayOriginal,
            },
        })
    }

    fn finalize_upstream_request(
        resource: &UpstreamResource,
        request_id: uuid::Uuid,
        headers: &mut HeaderMap,
        body: Option<&mut Bytes>,
    ) -> AppResult<()> {
        match resource.kind {
            UpstreamResourceKind::Account => {
                let body = body.ok_or_else(|| AppError::ProviderUpstream {
                    provider: PROVIDER.to_owned(),
                    message: format!(
                        "Claude OAuth 账号请求体未按草稿要求物化: resource_id={}",
                        resource.id
                    ),
                })?;
                let context = resource.parse_request_context::<ClaudeAccountRequestContext>()?;
                let original_body_bytes = body.len();
                let injected =
                    claude_request::inject_oauth_account_uuid(body, context.account_uuid)?;
                let upstream_body_bytes = injected.body.len();
                let original_metadata_user_id_kind = injected.original_user_id_kind;
                *body = injected.body;
                claude_header::inject_oauth_credential(headers, &resource.auth_secret)?;
                info!(
                    request_id = %request_id,
                    resource_type = resource.kind.as_str(),
                    resource_id = %resource.id,
                    original_body_bytes,
                    upstream_body_bytes,
                    original_metadata_user_id_kind,
                    "Claude adapter 已在单次请求最终化中注入 OAuth header 与 metadata attribution"
                );
            }
            UpstreamResourceKind::ApiKey => {
                claude_header::inject_official_api_key_credential(headers, &resource.auth_secret)?;
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

    if status.is_success() && is_sse_response(&headers) {
        let rate_limit_resets_at = claude_response::parse_rate_limit_reset(&headers);
        info!(
            resource_id = %resource.id,
            upstream_status = status.as_u16(),
            "Claude 上游返回 SSE，开始原始字节透传和协议旁路观察"
        );
        return Ok(ProtocolResponse::Streaming(StreamingProtocolResponse {
            status,
            headers: filtered_response_headers(&headers),
            stream: Box::pin(upstream_response.bytes_stream()),
            observer: Some(Box::new(ClaudeSseObserver::new(rate_limit_resets_at))),
        }));
    }

    let body = read_buffered_upstream_body(config, attempt.provider, upstream_response).await?;

    if status.is_success() {
        let usage = claude_response::parse_non_stream_usage(&body).map(|usage| {
            let usage = usage.into_token_usage();
            info!(
                resource_id = %resource.id,
                input_tokens = usage.input_tokens,
                cached_input_tokens = usage.cached_input_tokens,
                output_tokens = usage.output_tokens,
                reasoning_output_tokens = usage.reasoning_output_tokens,
                total_tokens = usage.total_tokens,
                "Claude 非流式 Messages 响应 token usage 已解析"
            );
            usage
        });
        if usage.is_none() {
            if let Some(input_tokens) = claude_response::parse_count_tokens(&body) {
                // count_tokens 是对后续模型请求的输入估算，不是该 HTTP 调用实际产生的
                // 模型 token 消耗，因此不能写入 request usage 或扣减用户额度。
                info!(
                    resource_id = %resource.id,
                    input_tokens,
                    "Claude count_tokens 成功响应已解析，不计入实际 token 消耗"
                );
            } else {
                warn!(
                    resource_id = %resource.id,
                    response_bytes = body.len(),
                    "Claude 非流式成功响应缺少可解析 usage，保持原始响应透传"
                );
            }
        }
        let response_headers = filtered_response_headers(&headers);
        return Ok(ProtocolResponse::Buffered(BufferedProtocolResponse {
            status,
            headers: response_headers,
            body,
            record_error_response: false,
            retry: false,
            exclude_resource_on_retry: false,
            feedback: None,
            usage,
        }));
    }

    let parsed_error = claude_response::parse_error(&body);
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
        upstream_request_id = parsed_error
            .as_ref()
            .and_then(|error| error.request_id.as_deref())
            .unwrap_or("<missing>"),
        error_type = parsed_error
            .as_ref()
            .map(|error| error.error.error_type.as_str())
            .unwrap_or("<unparsed>"),
        error_message = parsed_error
            .as_ref()
            .map(|error| truncate_log_value(&error.error.message, 512))
            .unwrap_or("<unparsed>"),
        response_bytes = body.len(),
        response_type = parsed_error
            .as_ref()
            .and_then(|error| error.response_type.as_deref())
            .unwrap_or("<missing>"),
        upstream_response_body_encoding = tracing_body.encoding(),
        upstream_response_body = %tracing_body.content(),
        "Claude 上游返回失败响应，已完成协议分类并把完整响应正文写入 tracing"
    );

    let signal = claude_response::classify_resource_failure(
        status,
        &headers,
        parsed_error.as_ref(),
        config.claude_account_rate_limit_cooldown_seconds,
    );
    // 只有未识别为凭证/权限/计费/限流信号、但协议明确要求重试的瞬态失败，才使用
    // 请求级资源排除；这不会修改 Key 或账号的全局维护状态。408 由调用方要求直接
    // 重试，因此即使被识别为瞬态失败，也不加入本请求排除集合。
    let transient_retry =
        signal.is_none() && claude_response::is_transient(status, &headers, parsed_error.as_ref());
    let (retry, feedback) = if resource.kind == UpstreamResourceKind::ApiKey {
        // 官方 Key 只有能够明确归因到凭证、权限、计费或该 Key 配额的失败才提交资源
        // 回执。网络波动和 Anthropic 临时故障只切换资源重试，不能因为一次公共上游故障
        // 把健康 Key 摘除；普通请求级 4xx 同样原样返回，避免调用方参数错误误伤资源。
        match signal {
            Some(claude_response::ResourceFailureSignal::AuthenticationRejected) => (
                true,
                Some(UpstreamFeedback::Error {
                    reason: "authentication_error".to_owned(),
                }),
            ),
            Some(claude_response::ResourceFailureSignal::RateLimited { .. }) => (
                true,
                Some(UpstreamFeedback::Error {
                    reason: "rate_limit_error".to_owned(),
                }),
            ),
            Some(claude_response::ResourceFailureSignal::BillingRejected) => (
                true,
                Some(UpstreamFeedback::Error {
                    reason: "billing_error".to_owned(),
                }),
            ),
            Some(claude_response::ResourceFailureSignal::PermissionDenied) => (
                true,
                Some(UpstreamFeedback::Error {
                    reason: "permission_error".to_owned(),
                }),
            ),
            None => (transient_retry, None),
        }
    } else {
        match signal {
            Some(claude_response::ResourceFailureSignal::AuthenticationRejected) => (
                true,
                Some(UpstreamFeedback::AuthenticationRejected {
                    reason: "authentication_error".to_owned(),
                }),
            ),
            Some(claude_response::ResourceFailureSignal::RateLimited { resets_at }) => (
                true,
                Some(UpstreamFeedback::RateLimited {
                    resets_at: Some(resets_at),
                    reason: "rate_limit_error".to_owned(),
                }),
            ),
            Some(claude_response::ResourceFailureSignal::BillingRejected) => (
                true,
                Some(UpstreamFeedback::EntitlementMissing {
                    reason: "billing_error".to_owned(),
                }),
            ),
            Some(claude_response::ResourceFailureSignal::PermissionDenied) => (
                true,
                Some(UpstreamFeedback::EntitlementMissing {
                    reason: "permission_error".to_owned(),
                }),
            ),
            None if transient_retry => (
                true,
                Some(UpstreamFeedback::TemporarilyUnavailable {
                    reason: parsed_error
                        .as_ref()
                        .map(|error| error.error.error_type.clone())
                        .unwrap_or_else(|| format!("HTTP {status}")),
                }),
            ),
            None => (false, None),
        }
    };
    let exclude_resource_on_retry = transient_retry && status != StatusCode::REQUEST_TIMEOUT;
    debug!(
        upstream_status = status.as_u16(),
        resource_type = resource.kind.as_str(),
        retry,
        transient_retry,
        exclude_resource_on_retry,
        feedback = feedback
            .as_ref()
            .map(UpstreamFeedback::as_str)
            .unwrap_or("none"),
        "Claude HTTP 失败已完成重试与请求级资源排除分类"
    );
    let response_headers = filtered_response_headers(&headers);
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

fn is_sse_response(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(';')
                .next()
                .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("text/event-stream"))
        })
}

fn filtered_response_headers(source: &HeaderMap) -> HeaderMap {
    let mut headers = HeaderMap::new();
    claude_response::copy_response_headers(source, &mut headers);
    headers
}

/// Claude SSE observer 只解析 wire event 并累计 usage、日志错误与中立上游事实。
struct ClaudeSseObserver {
    sse_buffer: Vec<u8>,
    usage: Option<claude_response::ClaudeUsage>,
    error: Option<StreamErrorRecord>,
    feedback_emitted: bool,
    rate_limit_resets_at: Option<DateTime<Utc>>,
}

impl ClaudeSseObserver {
    fn new(rate_limit_resets_at: Option<DateTime<Utc>>) -> Self {
        Self {
            sse_buffer: Vec::with_capacity(8192),
            usage: None,
            error: None,
            feedback_emitted: false,
            rate_limit_resets_at,
        }
    }

    fn process_buffered_events(&mut self) -> StreamUpdate {
        let mut update = StreamUpdate::default();
        while let Some((event_end, delimiter_len)) =
            claude_response::find_sse_event_boundary(&self.sse_buffer)
        {
            let mut event = self.sse_buffer.drain(..event_end).collect::<Vec<_>>();
            event.extend(self.sse_buffer.drain(..delimiter_len));
            let bytes = Bytes::from(event);
            let feedback = self.inspect_event(&bytes);
            update.output.push_back(bytes);
            if update.feedback.is_none() {
                update.feedback = feedback;
            }
        }

        if self.sse_buffer.len() > MAX_SSE_ITEM_BYTES {
            warn!(
                buffered_bytes = self.sse_buffer.len(),
                max_sse_item_bytes = MAX_SSE_ITEM_BYTES,
                "Claude SSE 单事件缓冲超过上限，当前缓冲按原始字节透传"
            );
            update
                .output
                .push_back(Bytes::from(std::mem::take(&mut self.sse_buffer)));
        }
        update
    }

    fn inspect_event(&mut self, bytes: &Bytes) -> Option<UpstreamFeedback> {
        let data = claude_response::collect_sse_event_data(bytes)?;
        match claude_response::parse_sse_data(&data) {
            Some(claude_response::SseData::Usage(usage)) => {
                self.usage = Some(self.usage.map_or(usage, |current| current.merge(usage)));
                None
            }
            Some(claude_response::SseData::Error(upstream_error)) => {
                let error_type = upstream_error.error_type;
                let error_message = truncate_log_value(&upstream_error.message, 512);
                let tracing_body = response_body_for_tracing(bytes);
                self.error = Some(StreamErrorRecord {
                    kind: "sse_event",
                    body: String::from_utf8_lossy(&data).to_string(),
                });
                let feedback = if self.feedback_emitted {
                    None
                } else {
                    stream_feedback(&error_type, self.rate_limit_resets_at)
                };
                if feedback.is_some() {
                    self.feedback_emitted = true;
                }
                warn!(
                    error_type,
                    error_message,
                    feedback = feedback
                        .as_ref()
                        .map(UpstreamFeedback::as_str)
                        .unwrap_or("none"),
                    upstream_response_body_bytes = bytes.len(),
                    upstream_response_body_encoding = tracing_body.encoding(),
                    upstream_response_body = %tracing_body.content(),
                    "Claude SSE error 已解析，完整原始事件已写入 tracing 并保持透传"
                );
                feedback
            }
            Some(claude_response::SseData::Other) | None => None,
        }
    }
}

impl StreamObserver for ClaudeSseObserver {
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
            usage: self.usage.take().map(|usage| usage.into_token_usage()),
            error: self.error.take(),
        }
    }
}

fn stream_feedback(
    error_type: &str,
    rate_limit_resets_at: Option<DateTime<Utc>>,
) -> Option<UpstreamFeedback> {
    match error_type {
        "authentication_error" => Some(UpstreamFeedback::AuthenticationRejected {
            reason: error_type.to_owned(),
        }),
        "rate_limit_error" => Some(UpstreamFeedback::RateLimited {
            resets_at: rate_limit_resets_at,
            reason: error_type.to_owned(),
        }),
        "api_error" | "overloaded_error" | "timeout_error" => {
            Some(UpstreamFeedback::TemporarilyUnavailable {
                reason: error_type.to_owned(),
            })
        }
        _ => None,
    }
}

fn truncate_log_value(value: &str, max_chars: usize) -> &str {
    value
        .char_indices()
        .nth(max_chars)
        .map_or(value, |(index, _)| &value[..index])
}
