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
                header as codex_header,
                response::{self as codex_response},
            },
            images,
            maintenance::GptMaintenance,
            model::GptAccountRequestContext,
            upstream::{build_upstream_url, classify_http_failure, filtered_response_headers},
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

/// OpenAI `/v1/images/generations` 的 GPT operation adapter。
///
/// 通用 gateway 继续负责鉴权、模型白名单、请求缓存、资源调度、重试、maintenance、额度
/// 和日志生命周期。本 adapter 只处理 Account/API Key 两种资源在 Images HTTP 协议上的差异。
pub struct GptImageGenerationsProxy;

impl ProviderProtocol for GptImageGenerationsProxy {
    type Maintenance = GptMaintenance;

    fn inspect_request(
        _headers: &HeaderMap,
        body: Bytes,
    ) -> impl Future<Output = AppResult<RequestInspection>> + Send {
        let result = images::inspect_generations_body(&body)
            .map(str::to_owned)
            .map(|requested_model| RequestInspection {
                requested_model,
                sticky_key: None,
                log_fields: RequestLogFields::default(),
            })
            .map_err(|message| AppError::BadRequest { message });
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
                &config.gpt_upstream_image_generations_path,
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
        let (headers, body) = match resource.kind {
            UpstreamResourceKind::Account => (
                codex_header::build_codex_image_upstream_headers(&request.headers),
                UpstreamRequestBodyMode::MaterializeOriginal,
            ),
            UpstreamResourceKind::ApiKey => (
                codex_header::build_official_api_key_upstream_headers(&request.headers),
                // OpenAI Images 在 model 缺失时可能选择其他默认模型。两类资源都物化并
                // 写入 gpt-image-2，确保调度结果不会改变调用方实际使用的模型。
                UpstreamRequestBodyMode::MaterializeOriginal,
            ),
        };

        info!(
            request_id = %request.request_id,
            resource_type = resource.kind.as_str(),
            resource_id = %resource.id,
            upstream_url = %target.url,
            query_passthrough = request.uri.query().is_some(),
            body_bytes = request.body.len(),
            body_materialization_required = matches!(&body, UpstreamRequestBodyMode::MaterializeOriginal),
            "GPT Images generations adapter 已生成上游请求草稿"
        );
        Ok(UpstreamRequestDraft {
            client_profile: target.client_profile,
            method: target.method,
            url: target.url,
            headers,
            body,
        })
    }

    fn finalize_upstream_request(
        resource: &UpstreamResource,
        request_id: uuid::Uuid,
        headers: &mut HeaderMap,
        body: Option<&mut Bytes>,
    ) -> AppResult<()> {
        let body = body.ok_or_else(|| AppError::ProviderUpstream {
            provider: Self::provider_name().to_owned(),
            message: "GPT Images 请求最终化时缺少已物化 body".to_owned(),
        })?;
        *body = images::transform_generations_body(body).map_err(|message| {
            AppError::ProviderUpstream {
                provider: Self::provider_name().to_owned(),
                message: format!("资源 override 后的图片请求无法转换: {message}"),
            }
        })?;
        // 两种资源都调用 JSON Images 端点；在管理员 override 之后恢复权威协议值。
        codex_header::apply_codex_image_protocol_headers(headers);

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
        info!(
            request_id = %request_id,
            resource_type = resource.kind.as_str(),
            resource_id = %resource.id,
            upstream_model = images::CODEX_IMAGE_MODEL,
            transformed_body_bytes = body.len(),
            "GPT Images 请求已归一化为 gpt-image-2 JSON 并注入真实资源凭证"
        );
        Ok(())
    }

    fn handle_response<'a>(
        config: &'a AppConfig,
        resource: &'a UpstreamResource,
        attempt: UpstreamAttemptContext,
        response: reqwest::Response,
    ) -> impl Future<Output = Result<ProtocolResponse, ProtocolFailure>> + Send + 'a {
        process_image_upstream_response(config, resource, attempt, response)
    }
}

pub(super) async fn process_image_upstream_response(
    config: &AppConfig,
    resource: &UpstreamResource,
    attempt: UpstreamAttemptContext,
    upstream_response: reqwest::Response,
) -> Result<ProtocolResponse, ProtocolFailure> {
    let status = upstream_response.status();
    let headers = upstream_response.headers().clone();
    let body = read_buffered_upstream_body(config, attempt.provider, upstream_response).await?;

    if status.is_success() {
        let usage = images::parse_image_usage(&body).map_err(|message| {
            ProtocolFailure::adapter(AppError::ProviderUpstream {
                provider: attempt.provider.to_owned(),
                message: format!("解析 GPT Images 成功响应 usage 失败: {message}"),
            })
        })?;
        let downstream_body = match resource.kind {
            UpstreamResourceKind::Account => images::transform_account_image_response(&body)
                .map_err(|message| {
                    ProtocolFailure::adapter(AppError::ProviderUpstream {
                        provider: attempt.provider.to_owned(),
                        message: format!("转换 Codex Images 成功响应失败: {message}"),
                    })
                })?,
            UpstreamResourceKind::ApiKey => body,
        };
        info!(
            request_id = %attempt.request_id,
            provider = attempt.provider,
            resource_type = resource.kind.as_str(),
            resource_id = %resource.id,
            upstream_status = status.as_u16(),
            response_bytes = downstream_body.len(),
            usage_present = usage.is_some(),
            "GPT Images 上游成功响应已完成 buffered 协议处理"
        );
        return Ok(ProtocolResponse::Buffered(
            BufferedProtocolResponse::Respond {
                status,
                headers: filtered_response_headers(&headers, resource.kind),
                body: downstream_body,
                feedback: None,
                usage,
            },
        ));
    }

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
        "GPT Images 上游返回失败响应，完整响应正文已写入 tracing"
    );
    let account_signal = codex_response::parse_account_signal(status, &body);
    let classification = classify_http_failure(resource.kind, status, account_signal);
    let response = if classification.retry {
        BufferedProtocolResponse::Retry {
            upstream_status: status,
            exclude_current_resource: classification.exclude_resource_on_retry,
            feedback: classification.feedback,
        }
    } else {
        BufferedProtocolResponse::Respond {
            status,
            headers: filtered_response_headers(&headers, resource.kind),
            body,
            feedback: classification.feedback,
            usage: None,
        }
    };
    Ok(ProtocolResponse::Buffered(response))
}
