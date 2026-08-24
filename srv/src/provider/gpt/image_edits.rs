use std::future::Future;

use axum::{
    body::Bytes,
    http::{HeaderMap, header},
};
use tracing::info;

use crate::{
    config::AppConfig,
    err::{AppError, AppResult},
    infra::http_client::HttpClientProfile,
    provider::{
        gpt::{
            codex_http::{header as codex_header, response as codex_response},
            image_generations::process_image_upstream_response,
            images,
            maintenance::GptMaintenance,
            model::GptAccountRequestContext,
            upstream::build_upstream_url,
        },
        protocol::{
            EncodedProviderError, ProtocolFailure, ProtocolResponse, ProviderProtocol,
            ProviderVisibleError, ReplayableRequest, RequestInspection, RequestLogFields,
            UpstreamAttemptContext, UpstreamRequestBodyMode, UpstreamRequestDraft,
            UpstreamRequestTarget,
        },
        resource::{UpstreamResource, UpstreamResourceKind},
    },
};

/// OpenAI `/v1/images/edits` 的 GPT operation adapter。
///
/// 调用方使用 multipart；Codex OAuth Account 和 OpenAI Official API Key 上游都使用
/// `images[].image_url` JSON。adapter 在 body override 前完成统一转换，因而混合资源组
/// 共享相同的模型、参数校验、override、重试、额度和日志语义。
pub struct GptImageEditsProxy;

impl ProviderProtocol for GptImageEditsProxy {
    type Maintenance = GptMaintenance;

    fn inspect_request(
        headers: &HeaderMap,
        body: Bytes,
    ) -> impl Future<Output = AppResult<RequestInspection>> + Send {
        let content_type = multipart_content_type(headers).map(str::to_owned);
        async move {
            let content_type = content_type.map_err(|message| AppError::BadRequest { message })?;
            let requested_model = images::inspect_edits_body(&content_type, body)
                .await
                .map_err(|message| AppError::BadRequest { message })?
                .to_owned();
            Ok(RequestInspection {
                requested_model,
                sticky_key: None,
                log_fields: RequestLogFields::default(),
            })
        }
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
                &config.gpt_upstream_image_edits_path,
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
            UpstreamResourceKind::Account => {
                codex_header::build_codex_image_upstream_headers(&request.headers)
            }
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
            "GPT Images edits adapter 已生成上游请求草稿"
        );
        Ok(UpstreamRequestDraft {
            client_profile: target.client_profile,
            method: target.method,
            url: target.url,
            headers,
            // 两种资源都先物化为 JSON 中间结构，确保 body override 不需要理解调用方
            // multipart boundary，并在 Account 与 Official API Key 上具有同一语义。
            body: UpstreamRequestBodyMode::MaterializeOriginal,
        })
    }

    fn transform_body_before_override(
        resource: &UpstreamResource,
        request: &ReplayableRequest,
        body: Bytes,
    ) -> impl Future<Output = AppResult<Bytes>> + Send {
        let content_type = multipart_content_type(&request.headers).map(str::to_owned);
        let resource_id = resource.id;
        async move {
            let content_type = content_type.map_err(|message| AppError::ProviderUpstream {
                provider: Self::provider_name().to_owned(),
                message: format!(
                    "图片编辑请求通过调度前检查后丢失 multipart Content-Type: resource_id={resource_id}, {message}"
                ),
            })?;
            images::transform_edits_multipart_body(&content_type, body)
                .await
                .map_err(|message| AppError::ProviderUpstream {
                    provider: Self::provider_name().to_owned(),
                    message: format!(
                        "图片编辑请求通过调度前检查后无法再次转换: resource_id={resource_id}, {message}"
                    ),
                })
        }
    }

    fn finalize_upstream_request(
        resource: &UpstreamResource,
        request_id: uuid::Uuid,
        headers: &mut HeaderMap,
        body: Option<&mut Bytes>,
    ) -> AppResult<()> {
        let body = body.ok_or_else(|| AppError::ProviderUpstream {
            provider: Self::provider_name().to_owned(),
            message: "GPT Images edits 请求最终化时缺少已物化 body".to_owned(),
        })?;
        let intermediate_body_bytes = body.len();
        let finalized =
            images::finalize_edits_body(body).map_err(|message| AppError::ProviderUpstream {
                provider: Self::provider_name().to_owned(),
                message: format!("资源 override 后的图片编辑请求无效: {message}"),
            })?;
        let image_count = finalized.image_count;
        *body = finalized.body;
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
            image_count,
            intermediate_body_bytes,
            transformed_body_bytes = body.len(),
            "GPT Images edits 请求已归一化为 gpt-image-2 JSON 并注入真实资源凭证"
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

fn multipart_content_type(headers: &HeaderMap) -> Result<&str, String> {
    let value = headers
        .get(header::CONTENT_TYPE)
        .ok_or_else(|| "图片编辑请求缺少 Content-Type".to_owned())?
        .to_str()
        .map_err(|error| format!("图片编辑请求 Content-Type 不是合法文本: {error}"))?;
    if !value.split(';').next().is_some_and(|media_type| {
        media_type
            .trim()
            .eq_ignore_ascii_case("multipart/form-data")
    }) {
        return Err("图片编辑请求 Content-Type 必须是 multipart/form-data".to_owned());
    }
    Ok(value)
}
