//! 模型网关的公开错误边界。只在这里将内部错误脱敏，再交给 Provider 编码。
use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};

use crate::{
    err::AppError,
    provider::protocol::{ProviderVisibleError, ProviderVisibleErrorKind},
};

/// 客户端在上传完成前断开连接时，以空 499 结束 HTTP handler。
pub(super) const CLIENT_CLOSED_REQUEST: StatusCode = match StatusCode::from_u16(499) {
    Ok(status) => status,
    Err(_) => panic!("499 必须是有效的 HTTP 状态码"),
};

/// 控制台错误只按整个分组处理，新增管理业务错误无需修改模型网关或 Provider。
/// 不通过通配分支吞掉新增网关错误，保留编译器的穷尽检查。
pub(super) fn project_error(error: &AppError) -> ProviderVisibleError {
    use ProviderVisibleErrorKind as Kind;
    let (status, kind, code) = match error {
        AppError::MissingApiKey | AppError::InvalidApiKey | AppError::DisabledApiKey => (
            StatusCode::UNAUTHORIZED,
            Kind::Authentication,
            "invalid_api_key",
        ),
        AppError::ModelNotAllowed { .. } => {
            (StatusCode::FORBIDDEN, Kind::Permission, "model_not_allowed")
        }
        AppError::GatewayKeyProviderMismatch { .. } => (
            StatusCode::FORBIDDEN,
            Kind::Permission,
            "gateway_key_provider_mismatch",
        ),
        AppError::GatewayKeyGroupUnavailable => (
            StatusCode::FORBIDDEN,
            Kind::Permission,
            "gateway_key_group_unavailable",
        ),
        AppError::UserQuotaExceeded => (
            StatusCode::TOO_MANY_REQUESTS,
            Kind::RateLimit,
            "user_quota_exceeded",
        ),
        AppError::UserConcurrencyExceeded { .. } => (
            StatusCode::TOO_MANY_REQUESTS,
            Kind::RateLimit,
            "user_concurrency_exceeded",
        ),
        AppError::BadRequest { .. } => (
            StatusCode::BAD_REQUEST,
            Kind::InvalidRequest,
            "invalid_request_error",
        ),
        AppError::PluginRequestRejected { code, .. } => {
            (StatusCode::BAD_REQUEST, Kind::InvalidRequest, code.as_str())
        }
        AppError::PayloadTooLarge { .. } => (
            StatusCode::PAYLOAD_TOO_LARGE,
            Kind::InvalidRequest,
            "request_too_large",
        ),
        AppError::RequestBodyInterrupted { .. } => (
            CLIENT_CLOSED_REQUEST,
            Kind::InvalidRequest,
            "request_body_interrupted",
        ),
        AppError::ResourceError { .. } => (
            StatusCode::SERVICE_UNAVAILABLE,
            Kind::Gateway,
            "resource_error",
        ),
        AppError::ProviderUpstream { .. } | AppError::Plugin { .. } | AppError::Email { .. } => {
            (StatusCode::BAD_GATEWAY, Kind::Gateway, "gateway_error")
        }
        AppError::ReadConfig { .. }
        | AppError::InvalidConfig { .. }
        | AppError::MissingConfig { .. }
        | AppError::Startup { .. }
        | AppError::HttpClientBuild { .. }
        | AppError::DbPoolBuild { .. }
        | AppError::DbPoolGet { .. }
        | AppError::DbQuery { .. }
        | AppError::RedisClient { .. }
        | AppError::Redis { .. }
        | AppError::Console(_)
        | AppError::BodyCache { .. }
        | AppError::ProviderStateSyncFailed { .. } => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Kind::Gateway,
            "gateway_error",
        ),
    };
    let message = match error {
        AppError::RequestBodyInterrupted { .. } => "request body interrupted".to_owned(),
        AppError::PluginRequestRejected { message, .. } => message.clone(),
        _ if kind == Kind::Gateway => "gateway error".to_owned(),
        _ => error.to_string(),
    };
    ProviderVisibleError {
        status,
        kind,
        code: code.to_owned(),
        message,
    }
}

/// 已注册路由却无法识别 Provider 表示内部路由声明不一致；普通未知路由由 Router 返回 404。
/// 此时没有可选择的 Provider encoder，仍在网关边界返回脱敏错误，不能借用控制台响应。
pub(super) fn unidentified_endpoint_response(error: AppError) -> Response {
    tracing::error!(error_code = error.code(), error_message = %error,
        http_status = 500, response_audience = "gateway", "模型网关无法识别已注册端点");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({
            "error": { "code": "gateway_error", "message": "gateway error" }
        })),
    )
        .into_response()
}
