//! 控制台 HTTP 错误边界：状态、公开信息、脱敏详情和诊断日志。
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;
use serde_json::{Value, json};
use tracing::error;

use crate::err::{AppError, ConsoleError};

pub(crate) type ConsoleResult<T> = Result<T, ConsoleApiError>;

/// 仅控制台 handler 和鉴权 extractor 使用；业务层继续返回 AppError。
#[derive(Debug)]
pub(crate) struct ConsoleApiError(AppError);

impl From<AppError> for ConsoleApiError {
    fn from(error: AppError) -> Self {
        Self(error)
    }
}

#[derive(Debug, Serialize)]
struct ErrorResponse<'a> {
    error: ErrorBody<'a>,
}

#[derive(Debug, Serialize)]
struct ErrorBody<'a> {
    code: &'a str,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<Value>,
}

impl ConsoleApiError {
    fn status_code(&self) -> StatusCode {
        match &self.0 {
            AppError::ReadConfig { .. }
            | AppError::InvalidConfig { .. }
            | AppError::MissingConfig { .. } => StatusCode::INTERNAL_SERVER_ERROR,
            AppError::Startup { .. } => StatusCode::INTERNAL_SERVER_ERROR,
            AppError::DbPoolBuild { .. }
            | AppError::DbPoolGet { .. }
            | AppError::DbQuery { .. }
            | AppError::RedisClient { .. }
            | AppError::Redis { .. } => StatusCode::INTERNAL_SERVER_ERROR,
            AppError::MissingApiKey | AppError::InvalidApiKey | AppError::DisabledApiKey => {
                StatusCode::UNAUTHORIZED
            }
            AppError::UserQuotaExceeded | AppError::UserConcurrencyExceeded { .. } => {
                StatusCode::TOO_MANY_REQUESTS
            }
            AppError::ModelNotAllowed { .. }
            | AppError::GatewayKeyProviderMismatch { .. }
            | AppError::GatewayKeyGroupUnavailable => StatusCode::FORBIDDEN,
            AppError::Console(error) => match error {
                ConsoleError::MissingDashboardToken | ConsoleError::InvalidDashboardToken => {
                    StatusCode::UNAUTHORIZED
                }
                ConsoleError::Forbidden | ConsoleError::TenantWasmUploadForbidden => {
                    StatusCode::FORBIDDEN
                }
                ConsoleError::TenantUserLimitExceeded { .. }
                | ConsoleError::TenantGatewayKeyLimitExceeded { .. } => StatusCode::CONFLICT,
            },
            AppError::BadRequest { .. } | AppError::PluginRequestRejected { .. } => {
                StatusCode::BAD_REQUEST
            }
            AppError::PayloadTooLarge { .. } => StatusCode::PAYLOAD_TOO_LARGE,
            AppError::RequestBodyInterrupted { .. } => {
                StatusCode::from_u16(499).expect("499 是合法状态码")
            }
            AppError::BodyCache { .. } => StatusCode::INTERNAL_SERVER_ERROR,
            AppError::ResourceError { .. } => StatusCode::SERVICE_UNAVAILABLE,
            AppError::ProviderUpstream { .. } | AppError::Plugin { .. } => StatusCode::BAD_GATEWAY,
            AppError::Email { .. } => StatusCode::BAD_GATEWAY,
            AppError::ProviderStateSyncFailed { .. } => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn response_body_bytes(&self) -> Vec<u8> {
        let payload = ErrorResponse {
            error: ErrorBody {
                code: self.0.code(),
                message: self.public_message(),
                details: self.safe_details(),
            },
        };

        serde_json::to_vec(&payload).unwrap_or_else(|_| {
            br#"{"error":{"code":"internal_server_error","message":"Internal server error"}}"#
                .to_vec()
        })
    }

    fn public_message(&self) -> String {
        match &self.0 {
            AppError::ReadConfig { .. }
            | AppError::InvalidConfig { .. }
            | AppError::MissingConfig { .. }
            | AppError::Startup { .. }
            | AppError::DbPoolBuild { .. }
            | AppError::DbPoolGet { .. }
            | AppError::DbQuery { .. }
            | AppError::RedisClient { .. }
            | AppError::Redis { .. }
            | AppError::Plugin { .. }
            | AppError::RequestBodyInterrupted { .. }
            | AppError::BodyCache { .. } => {
                "服务内部错误，请联系管理员并提供响应中的 x-request-id".to_owned()
            }
            AppError::Email { .. } => "邮件服务暂时不可用，请稍后重试".to_owned(),
            AppError::ResourceError { .. } | AppError::ProviderUpstream { .. } => {
                "上游服务请求失败，请稍后重试".to_owned()
            }
            AppError::ProviderStateSyncFailed { .. } => {
                "数据库操作已经完成，但 Redis runtime 更新或读取失败；请勿重复提交，管理员应检查日志并重建 runtime"
                    .to_owned()
            }
            _ => self.0.to_string(),
        }
    }

    /// 只返回不包含凭证、连接串或上游响应体的稳定上下文。
    fn safe_details(&self) -> Option<Value> {
        match &self.0 {
            AppError::ProviderStateSyncFailed {
                provider,
                resource_type,
                resource_id,
                ..
            } => Some(json!({
                "provider": provider,
                "resource_type": resource_type,
                "resource_id": resource_id,
                "database_committed": true,
                "replay_safe": false,
            })),
            _ => None,
        }
    }

    fn into_response_with_logging(self) -> Response {
        let status = self.status_code();
        let code = self.0.code();
        let diagnostic_message = self.0.to_string();

        error!(
            error_code = code,
            http_status = status.as_u16(),
            error_message = %diagnostic_message,
            response_audience = "console",
            "请求处理失败"
        );

        let body = self.response_body_bytes();
        (
            status,
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            body,
        )
            .into_response()
    }
}

impl IntoResponse for ConsoleApiError {
    fn into_response(self) -> Response {
        self.into_response_with_logging()
    }
}
