use std::future::Future;

use axum::{body::Bytes, http::HeaderMap};
use serde::Deserialize;
use tracing::{info, warn};

use crate::{
    config::AppConfig,
    err::{AppError, AppResult},
    infra::http_client::HttpClientProfile,
    provider::{
        gpt::{
            codex_http::{header as codex_header, response as codex_response},
            maintenance::GptMaintenance,
            model::GptAccountRequestContext,
            upstream::{build_upstream_url, filtered_response_headers},
        },
        protocol::{
            BufferedProtocolResponse, EncodedProviderError, ProtocolFailure, ProtocolResponse,
            ProviderProtocol, ProviderVisibleError, ReplayableRequest, RequestInspection,
            RequestLogFields, UpstreamAttemptContext, UpstreamRequestBodyMode,
            UpstreamRequestDraft, UpstreamRequestTarget, read_buffered_upstream_body,
        },
        resource::{UpstreamResource, UpstreamResourceKind},
        response_logging::response_body_for_tracing,
    },
};

/// Codex standalone search 请求中网关生命周期需要读取的最小字段集合。
///
/// 搜索命令、输入历史与 settings 均继续使用调用方原始 JSON 透传；这里不复制完整 DTO，
/// 只提取模型授权、同一搜索会话的资源粘性以及请求日志所需的 reasoning。
#[derive(Debug, Deserialize)]
struct SearchRequestMetadata {
    id: String,
    model: String,
    #[serde(default)]
    reasoning: Option<serde_json::Value>,
}

/// GPT `/v1/alpha/search` 的 provider/operation adapter。
///
/// Search 与 Responses 共享 GPT 资源池、凭证和 Codex 请求头语义，但上游响应是一次性 JSON。
/// 当前不解释任何错误正文，也不产生 token usage；所有 HTTP 状态和正文均直接交还调用方。
pub struct GptSearchProxy;

impl ProviderProtocol for GptSearchProxy {
    type Maintenance = GptMaintenance;

    fn inspect_request(
        _headers: &HeaderMap,
        body: Bytes,
    ) -> impl Future<Output = AppResult<RequestInspection>> + Send {
        let result = (|| {
            let metadata =
                serde_json::from_slice::<SearchRequestMetadata>(&body).map_err(|source| {
                    AppError::BadRequest {
                        message: format!("GPT Search 请求体元数据格式无效: {source}"),
                    }
                })?;
            let model = metadata.model.trim();
            if model.is_empty() {
                return Err(AppError::BadRequest {
                    message: "GPT Search 请求体 model 不能为空".to_owned(),
                });
            }
            let search_id = metadata.id.trim();
            if search_id.is_empty() {
                return Err(AppError::BadRequest {
                    message: "GPT Search 请求体 id 不能为空".to_owned(),
                });
            }
            let reasoning = metadata
                .reasoning
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .map_err(|source| AppError::BadRequest {
                    message: format!("GPT Search 请求体 reasoning 字段无法序列化: {source}"),
                })?;

            Ok(RequestInspection {
                requested_model: model.to_owned(),
                // Codex 在同一线程的 search/open/click/find 请求中复用 id。保持资源粘性可
                // 避免上游的搜索引用或加密上下文被切换到另一个账号后失效。
                sticky_key: Some(format!("search:id:{search_id}")),
                log_fields: RequestLogFields {
                    reasoning,
                    service_tier: None,
                    fast_mode: None,
                    is_compaction: Some(false),
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
        let (base_url, client_profile) = match resource.kind {
            UpstreamResourceKind::Account => (
                config.gpt_upstream_base_url.as_str(),
                HttpClientProfile::ChatGptCodex,
            ),
            UpstreamResourceKind::ApiKey => {
                (resource.api_key_base_url()?, HttpClientProfile::Generic)
            }
        };
        Ok(UpstreamRequestTarget {
            client_profile,
            method: reqwest::Method::POST,
            url: build_upstream_url(
                base_url,
                &config.gpt_upstream_search_path,
                request.uri.query(),
            ),
        })
    }

    fn prepare_upstream_request(
        config: &AppConfig,
        resource: &UpstreamResource,
        request: &ReplayableRequest,
    ) -> AppResult<UpstreamRequestDraft> {
        let target = Self::prepare_upstream_target(config, resource, request)?;
        let headers = match resource.kind {
            UpstreamResourceKind::Account => codex_header::build_codex_search_upstream_headers(
                &request.headers,
                config.codex_version_header.as_deref(),
            ),
            UpstreamResourceKind::ApiKey => {
                codex_header::build_official_api_key_upstream_headers(&request.headers)
            }
        };

        info!(
            request_id = %request.request_id,
            resource_type = resource.kind.as_str(),
            resource_id = %resource.id,
            upstream_url = %target.url,
            query_passthrough = request.uri.query().is_some(),
            body_bytes = request.body.len(),
            "GPT Search adapter 已生成上游请求草稿"
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
        // Search 固定使用 JSON。资源 header override 之后恢复协议值，再最后覆盖真实凭证，
        // 保证调用方和管理员配置都不能把网关凭证或错误 Content-Type 带到上游。
        codex_header::apply_codex_search_protocol_headers(headers);
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
    let body = read_buffered_upstream_body(config, attempt.provider, upstream_response).await?;

    if status.is_success() {
        info!(
            request_id = %attempt.request_id,
            provider = attempt.provider,
            resource_type = attempt.resource_kind.as_str(),
            resource_id = %attempt.resource_id,
            runtime_revision = attempt.runtime_revision,
            attempt_number = attempt.attempt_number,
            max_attempts = attempt.max_attempts,
            upstream_status = status.as_u16(),
            response_bytes = body.len(),
            "GPT Search 上游成功响应已完整读取，准备原样透传"
        );
    } else {
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
            "GPT Search 上游错误响应暂不分类，完整正文已写入 tracing 并将原样透传"
        );
    }

    Ok(ProtocolResponse::Buffered(
        BufferedProtocolResponse::Respond {
            status,
            headers: filtered_response_headers(&headers, resource.kind),
            body,
            feedback: None,
            usage: None,
        },
    ))
}
