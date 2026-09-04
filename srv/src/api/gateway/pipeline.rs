use axum::{
    body::Body,
    extract::Request,
    http::{Response, Uri},
};
use tracing::{error, info};

use crate::{
    err::{AppError, AppResult, CLIENT_CLOSED_REQUEST},
    provider::{
        protocol::{ProviderProtocol, ProviderVisibleError, ReplayableRequest},
        proxy,
    },
    request::{
        body_cache,
        concurrency::{self, AcquireResult},
        events::{
            GatewayAuthDetails, RequestEndResult, RequestEvent, RequestInspectionDetails,
            UsageAttribution,
        },
    },
    state::AppState,
};

use super::{auth, endpoint::EndpointDescriptor};

/// 执行 provider 共用的请求生命周期。
///
/// 固定顺序为：调用方 header 鉴权并发布网关归属、按用户和 provider 做并发准入、缓存
/// 原始 body、provider 检查元数据并发布协议字段、model 授权、通用调度和重试、provider
/// 响应处理、日志收尾。资源请求 override 与认证注入位于 provider 的单次 attempt 内，
/// 且每次重试都从此处保存的原始请求重新构造。
pub(super) async fn execute_pipeline<P>(
    state: &AppState,
    endpoint: EndpointDescriptor,
    uri: Uri,
    request: Request<Body>,
    request_id: uuid::Uuid,
) -> Response<Body>
where
    P: ProviderProtocol,
{
    let result = prepare_and_execute::<P>(state, endpoint, uri, request, request_id).await;
    finish_pipeline::<P>(state, endpoint, request_id, result).await
}

async fn prepare_and_execute<P>(
    state: &AppState,
    endpoint: EndpointDescriptor,
    uri: Uri,
    request: Request<Body>,
    request_id: uuid::Uuid,
) -> AppResult<Response<Body>>
where
    P: ProviderProtocol,
{
    let headers = request.headers().clone();
    let auth = auth::authenticate_gateway_key(
        state,
        &headers,
        P::provider_name(),
        endpoint.plugin_policy.enabled(),
    )
    .await?;
    // API Key、用户和分组均是网关领域事实，不应等待 body 上传或 provider 私有 DTO
    // 检查。这里在鉴权成功后立即发布归属，使后续请求体中断、超限或格式错误也能在
    // ClickHouse 中关联到真实调用方。
    state
        .request_events()
        .emit(RequestEvent::GatewayAuthenticated {
            request_id,
            details: GatewayAuthDetails {
                tenant_id: auth.tenant_id().to_owned(),
                api_key_id: auth.api_key_id(),
                api_key_name: auth.api_key_name().to_owned(),
                user_id: auth.user_id(),
                username: auth.username().to_owned(),
                provider_group_id: auth.group_id(),
                provider_group_name: auth.group_name().to_owned(),
            },
        });
    // 用户并发槽位按 provider 独立登记，并覆盖从请求体上传到最终响应 body 结束的完整
    // 生命周期。同一请求内部的上游重试发生在该 lease 内，不会重复占用用户并发。
    let concurrency_lease = match concurrency::acquire(
        state,
        request_id,
        auth.tenant_id().to_owned(),
        auth.user_id(),
        P::provider_name(),
        auth.max_concurrency(),
    )
    .await?
    {
        AcquireResult::Acquired(lease) => lease,
        AcquireResult::LimitExceeded { current, limit } => {
            return Err(AppError::UserConcurrencyExceeded {
                provider: P::provider_name().to_owned(),
                current,
                limit,
            });
        }
    };
    let execution: AppResult<Response<Body>> = async {
        let (body, inspection_bytes) = body_cache::cache_request_body(
            request,
            request_id,
            state.config().body_memory_limit_bytes,
        )
        .await?;

        // 无论是否绑定插件，inspection 始终描述调用方提交的原始 provider 请求，并在资源
        // 调度之前完成。插件是 admin 控制的上游请求改造能力，可以在后续 attempt 中静默
        // 修改最终 header/body，但不接管用户模型授权、请求日志字段或会话粘性语义。
        // Bytes clone 只增加引用计数；multipart adapter 可直接持有同一块完整请求体，避免
        // 图片上传在调度前检查时额外复制几十 MiB。
        let inspection = P::inspect_request(&headers, inspection_bytes.clone()).await?;
        let model = inspection.requested_model;
        let sticky_key = inspection.sticky_key;
        let log_fields = inspection.log_fields;
        state.request_events().emit(RequestEvent::RequestInspected {
            request_id,
            details: RequestInspectionDetails {
                model: model.clone(),
                log_fields: log_fields.clone(),
            },
        });
        auth::authorize_gateway_payload(&auth, Some(&model))?;

        // 只有插件 attempt 需要完整原始字节作为 Component 输入；原生路径在 inspection 完成
        // 后立即释放这份额外 Bytes 句柄，后续继续通过 CachedBody 进行零污染重放。
        let plugin_binding = auth.plugin().cloned();
        let plugin_original_body = if plugin_binding.as_ref().is_some_and(|binding| {
            binding
                .artifact(crate::plugin::model::PluginSlot::Request)
                .is_some()
        }) {
            Some(inspection_bytes)
        } else {
            drop(inspection_bytes);
            None
        };
        info!(
            request_id = %request_id,
            provider = P::provider_name(),
            method = "POST",
            uri = %uri,
            api_key_id = %auth.api_key_id(),
            api_key_name = auth.api_key_name(),
            user_id = %auth.user_id(),
            username = auth.username(),
            provider_group_id = %auth.group_id(),
            provider_group_name = %auth.group_name(),
            requested_model = %model,
            request_header_count = headers.len(),
            has_sticky_key = sticky_key.is_some(),
            has_reasoning = log_fields.reasoning.is_some(),
            service_tier = log_fields.service_tier.as_deref().unwrap_or("<none>"),
            fast_mode = ?log_fields.fast_mode,
            is_compaction = ?log_fields.is_compaction,
            plugin_release_id = ?plugin_binding.as_ref().map(|binding| binding.release_id),
            body_cache_request_id = %body.request_id(),
            body_cache_storage = body.storage_kind(),
            body_bytes = body.len(),
            body_memory_limit_bytes = state.config().body_memory_limit_bytes,
            "通用 gateway 预处理完成：原始请求已检查、模型已授权、请求体可重放"
        );
        let group_id = auth.group_id();
        let usage_attribution = UsageAttribution {
            user_id: auth.user_id(),
            api_key_id: auth.api_key_id(),
        };
        let response = proxy::execute::<P>(
            state,
            ReplayableRequest {
                request_id,
                uri,
                headers,
                body,
            },
            group_id,
            sticky_key,
            usage_attribution,
            plugin_binding,
            plugin_original_body,
        )
        .await
        .map_err(|error| {
            // `BadRequest` 在进入 proxy 之前只表示调用方 payload 校验失败；一旦已经完成
            // inspect/authorize，后续同名错误只可能来自资源运行态、override 或 credential
            // 构造。这里按阶段把这类歧义错误收口为内部 provider 故障，避免把资源配置细节
            // 当成调用方参数错误返回。完整诊断仍会由 finish_pipeline 写入 tracing。
            if let AppError::BadRequest { message } = error {
                return AppError::ProviderUpstream {
                    provider: P::provider_name().to_owned(),
                    message: format!("provider pipeline 内部请求构造失败: {message}"),
                };
            }
            error
        })?;

        Ok(response)
    }
    .await;

    match execution {
        Ok(response) => Ok(concurrency::hold_response(response, concurrency_lease)),
        Err(request_error) => {
            if let Err(release_error) = concurrency_lease.release().await {
                error!(
                    request_id = %request_id,
                    provider = P::provider_name(),
                    request_error_code = request_error.code(),
                    request_error = %request_error,
                    release_error = %release_error,
                    "provider pipeline 失败后同步释放用户并发 lease 失败；RAII guard 已提交兜底释放"
                );
            }
            Err(request_error)
        }
    }
}

async fn finish_pipeline<P: ProviderProtocol>(
    state: &AppState,
    endpoint: EndpointDescriptor,
    request_id: uuid::Uuid,
    result: AppResult<Response<Body>>,
) -> Response<Body> {
    match result {
        // Buffered 响应已经在最终透传位置发送原子终态事件；流式响应则只会在 body
        // 真正 EOF、错误、超时或被下游丢弃时发送终态。gateway 不再持有日志生命周期。
        Ok(response) => response,
        Err(error @ AppError::RequestBodyInterrupted { .. }) => {
            // 调用方已在请求体完整上传前断开，此时既无法也不应该再构造 provider 错误
            // 正文。空 499 只用于完成 Axum handler；请求日志通过 termination 明确记录
            // 真实结果，且不会伪造一份客户端从未收到的 HTTP error_response。
            let termination_kind = error.code();
            info!(
                request_id = %request_id,
                provider = endpoint.provider,
                operation = endpoint.operation.as_str(),
                route = endpoint.route,
                error_code = termination_kind,
                http_status = CLIENT_CLOSED_REQUEST.as_u16(),
                error_message = %error,
                "调用方在请求体完整读取前中断连接，provider pipeline 已停止"
            );
            state.request_events().emit(RequestEvent::Ended {
                request_id,
                occurred_at: chrono::Utc::now(),
                result: RequestEndResult::RequestBodyInterrupted,
            });

            let mut response = Response::new(Body::empty());
            *response.status_mut() = CLIENT_CLOSED_REQUEST;
            response
        }
        Err(error) => {
            // 先把完整内部错误写入 tracing，再通过公共投影决定调用方可见信息。具体
            // provider 只能拿到脱敏结果，无法在 wire encoder 中误用 `error.to_string()`。
            let visible_error = ProviderVisibleError::from_app_error(&error);
            error!(
                request_id = %request_id,
                provider = endpoint.provider,
                operation = endpoint.operation.as_str(),
                route = endpoint.route,
                error_code = error.code(),
                http_status = error.status_code().as_u16(),
                error_message = %error,
                provider_error_code = visible_error.code,
                provider_error_message = %visible_error.message,
                "通用 gateway 错误已转换为 provider 协议响应"
            );

            // provider 只执行一次序列化。ClickHouse 和 HTTP response 复用这里产出的同一
            // 份最终字节，因此 Dashboard 日志展示内容必然与模型调用方实际收到的 body 一致。
            let encoded = P::encode_error(&visible_error, request_id);
            let response_started_at = chrono::Utc::now();
            // 这是最终交给调用方的 buffered 错误响应已经完整就绪的时间；上游 attempt
            // 是否曾经返回响应头不会影响客户端视角的首字耗时。
            if !encoded.body.is_empty() {
                state.request_events().emit(RequestEvent::ResponseStarted {
                    request_id,
                    occurred_at: response_started_at,
                });
            }
            state.request_events().emit(RequestEvent::Ended {
                request_id,
                occurred_at: response_started_at,
                result: RequestEndResult::HttpFailure {
                    status_code: encoded.status.as_u16(),
                    body: encoded.body.clone(),
                },
            });
            encoded.into_response()
        }
    }
}
