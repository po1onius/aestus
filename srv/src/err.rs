use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;
use serde_json::{Value, json};
use thiserror::Error;
use tracing::error;
use uuid::Uuid;

pub type AppResult<T> = Result<T, AppError>;
pub type AdminResult<T> = Result<T, AdminError>;

const MAX_ADMIN_ERROR_MESSAGE_CHARS: usize = 4_096;

/// 非标准 499 状态码沿用网关领域的通用约定，表示调用方在服务返回响应前关闭了连接。
///
/// 客户端通常已经无法收到这个响应；保留明确状态只是为了让 Axum handler 完整收尾，
/// 同时避免把调用方主动中断误报成网关内部的 500 错误。
pub(crate) const CLIENT_CLOSED_REQUEST: StatusCode = match StatusCode::from_u16(499) {
    Ok(status) => status,
    Err(_) => panic!("499 必须是有效的 HTTP 状态码"),
};

#[derive(Debug, Error)]
pub enum AppError {
    #[error("读取配置失败: {key}")]
    ReadConfig {
        key: &'static str,
        #[source]
        source: std::env::VarError,
    },

    #[error("配置项 {key} 的值 {value} 无法解析")]
    InvalidConfig {
        key: &'static str,
        value: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    #[error("缺少配置项或配置为空: {key}")]
    MissingConfig { key: &'static str },

    #[error("服务启动失败: {message}")]
    Startup { message: String },

    #[error("数据库连接池初始化失败: {message}")]
    DbPoolBuild { message: String },

    #[error("数据库连接获取失败: {message}")]
    DbPoolGet { message: String },

    #[error("数据库查询失败: {message}")]
    DbQuery { message: String },

    #[error("Redis 客户端初始化失败: {message}")]
    RedisClient { message: String },

    #[error("Redis 操作失败: {message}")]
    Redis { message: String },

    #[error("缺少 Bearer API Key")]
    MissingApiKey,

    #[error("API Key 不可用")]
    InvalidApiKey,

    #[error("API Key 已禁用")]
    DisabledApiKey,

    #[error("用户 token 额度已用尽")]
    UserQuotaExceeded,

    #[error("API Key 无权调用模型: {model}")]
    ModelNotAllowed { model: String },

    #[error("API Key 分组属于 {key_provider}，不能调用 {requested_provider} 接口")]
    GatewayKeyProviderMismatch {
        key_provider: String,
        requested_provider: String,
    },

    #[error("API Key 所属 Provider 分组已归档")]
    GatewayKeyGroupUnavailable,

    #[error("缺少 Dashboard 登录凭证")]
    MissingDashboardToken,

    #[error("Dashboard 登录凭证无效")]
    InvalidDashboardToken,

    #[error("当前用户无权访问该资源")]
    Forbidden,

    #[error("请求参数无效: {message}")]
    BadRequest { message: String },

    /// 请求插件已经成功运行，并基于调用方输入主动拒绝继续构造上游请求。
    ///
    /// 该错误与 WASM trap、内存越界、非法插件输出等 `Plugin` 故障严格区分：前者是
    /// 调用方可以修正的请求错误，后者是网关或插件套件故障。`code/message` 来自受信任的
    /// 已发布插件，并由 runtime 在进入该类型前执行长度和空值收敛。
    #[error("请求插件拒绝处理: code={code}, message={message}")]
    PluginRequestRejected { code: String, message: String },

    #[error("请求体超过限制: {limit_bytes} bytes")]
    PayloadTooLarge { limit_bytes: usize },

    #[error("调用方请求体传输中断: {message}")]
    RequestBodyInterrupted { message: String },

    #[error("请求体缓存失败: {message}")]
    BodyCache { message: String },

    /// 网关已经完成调用方鉴权与请求检查，但资源池无法继续完成本次请求。
    ///
    /// `message` 只用于 tracing 诊断，可能描述“没有候选资源”或最后一次可重试上游
    /// attempt 的状态；provider 公共错误投影不会把它返回给模型调用方。
    #[error("上游资源错误: provider={provider}, group_id={group_id}: {message}")]
    ResourceError {
        provider: String,
        group_id: Uuid,
        message: String,
    },

    #[error("{provider} 上游请求失败: {message}")]
    ProviderUpstream { provider: String, message: String },

    #[error("WASM 插件套件执行失败: {message}")]
    Plugin { message: String },

    #[error("邮件发送失败: {message}")]
    Email { message: String },

    #[error(
        "provider 数据库操作已提交，但 Redis runtime 更新或读取失败: provider={provider}, resource_type={resource_type}, resource_id={resource_id}: {source}"
    )]
    ProviderStateSyncFailed {
        provider: &'static str,
        resource_type: &'static str,
        resource_id: Uuid,
        #[source]
        source: Box<AppError>,
    },
}

/// 已通过 admin 身份校验后的 handler 错误。
///
/// 普通 `AppError` 永远只返回经过脱敏的公共信息；admin handler 在鉴权 extractor 成功后
/// 使用本包装类型，才会在响应中携带截断后的技术诊断。原始错误始终完整写入服务日志。
#[derive(Debug)]
pub struct AdminError(AppError);

impl From<AppError> for AdminError {
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

#[derive(Debug, Clone, Copy)]
enum ErrorAudience {
    Public,
    Admin,
}

impl AppError {
    pub fn status_code(&self) -> StatusCode {
        match self {
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
            AppError::UserQuotaExceeded => StatusCode::TOO_MANY_REQUESTS,
            AppError::ModelNotAllowed { .. }
            | AppError::GatewayKeyProviderMismatch { .. }
            | AppError::GatewayKeyGroupUnavailable => StatusCode::FORBIDDEN,
            AppError::MissingDashboardToken | AppError::InvalidDashboardToken => {
                StatusCode::UNAUTHORIZED
            }
            AppError::Forbidden => StatusCode::FORBIDDEN,
            AppError::BadRequest { .. } | AppError::PluginRequestRejected { .. } => {
                StatusCode::BAD_REQUEST
            }
            AppError::PayloadTooLarge { .. } => StatusCode::PAYLOAD_TOO_LARGE,
            AppError::RequestBodyInterrupted { .. } => CLIENT_CLOSED_REQUEST,
            AppError::BodyCache { .. } => StatusCode::INTERNAL_SERVER_ERROR,
            AppError::ResourceError { .. } => StatusCode::SERVICE_UNAVAILABLE,
            AppError::ProviderUpstream { .. } | AppError::Plugin { .. } => StatusCode::BAD_GATEWAY,
            AppError::Email { .. } => StatusCode::BAD_GATEWAY,
            AppError::ProviderStateSyncFailed { .. } => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            AppError::ReadConfig { .. } => "read_config_failed",
            AppError::InvalidConfig { .. } => "invalid_config",
            AppError::MissingConfig { .. } => "missing_config",
            AppError::Startup { .. } => "startup_failed",
            AppError::DbPoolBuild { .. } => "db_pool_build_failed",
            AppError::DbPoolGet { .. } => "db_pool_get_failed",
            AppError::DbQuery { .. } => "db_query_failed",
            AppError::RedisClient { .. } => "redis_client_failed",
            AppError::Redis { .. } => "redis_failed",
            AppError::MissingApiKey => "missing_api_key",
            AppError::InvalidApiKey => "invalid_api_key",
            AppError::DisabledApiKey => "disabled_api_key",
            AppError::UserQuotaExceeded => "user_quota_exceeded",
            AppError::ModelNotAllowed { .. } => "model_not_allowed",
            AppError::GatewayKeyProviderMismatch { .. } => "gateway_key_provider_mismatch",
            AppError::GatewayKeyGroupUnavailable => "gateway_key_group_unavailable",
            AppError::MissingDashboardToken => "missing_dashboard_token",
            AppError::InvalidDashboardToken => "invalid_dashboard_token",
            AppError::Forbidden => "forbidden",
            AppError::BadRequest { .. } => "bad_request",
            AppError::PluginRequestRejected { .. } => "plugin_request_rejected",
            AppError::PayloadTooLarge { .. } => "payload_too_large",
            AppError::RequestBodyInterrupted { .. } => "request_body_interrupted",
            AppError::BodyCache { .. } => "body_cache_failed",
            AppError::ResourceError { .. } => "resource_error",
            AppError::ProviderUpstream { .. } => "provider_upstream_failed",
            AppError::Plugin { .. } => "plugin_failed",
            AppError::Email { .. } => "email_failed",
            AppError::ProviderStateSyncFailed { .. } => "provider_state_sync_failed",
        }
    }

    fn response_body_bytes_for(&self, audience: ErrorAudience) -> Vec<u8> {
        let payload = ErrorResponse {
            error: ErrorBody {
                code: self.code(),
                message: self.message_for(audience),
                details: self.safe_details(),
            },
        };

        serde_json::to_vec(&payload).unwrap_or_else(|_| {
            br#"{"error":{"code":"internal_server_error","message":"Internal server error"}}"#
                .to_vec()
        })
    }

    fn message_for(&self, audience: ErrorAudience) -> String {
        if matches!(audience, ErrorAudience::Admin) {
            return truncate_chars(&self.to_string(), MAX_ADMIN_ERROR_MESSAGE_CHARS);
        }

        match self {
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
            _ => self.to_string(),
        }
    }

    /// 只返回不包含凭证、连接串或上游响应体的稳定上下文。
    fn safe_details(&self) -> Option<Value> {
        match self {
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

    fn into_response_for(self, audience: ErrorAudience) -> Response {
        let status = self.status_code();
        let code = self.code();
        let diagnostic_message = self.to_string();

        error!(
            error_code = code,
            http_status = status.as_u16(),
            error_message = %diagnostic_message,
            response_audience = match audience {
                ErrorAudience::Public => "public",
                ErrorAudience::Admin => "admin",
            },
            "请求处理失败"
        );

        let body = self.response_body_bytes_for(audience);
        (
            status,
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            body,
        )
            .into_response()
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        self.into_response_for(ErrorAudience::Public)
    }
}

impl IntoResponse for AdminError {
    fn into_response(self) -> Response {
        self.0.into_response_for(ErrorAudience::Admin)
    }
}

impl From<diesel::result::Error> for AppError {
    fn from(source: diesel::result::Error) -> Self {
        Self::DbQuery {
            message: source.to_string(),
        }
    }
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}
