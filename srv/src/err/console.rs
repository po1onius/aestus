//! 控制台业务错误。新增管理功能错误只扩展本枚举及控制台响应映射。
//! 这里只描述业务事实，不决定 HTTP 状态或 Provider 协议。
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConsoleError {
    #[error("缺少 Dashboard 登录凭证")]
    MissingDashboardToken,

    #[error("Dashboard 登录凭证无效")]
    InvalidDashboardToken,

    #[error("当前用户无权访问该资源")]
    Forbidden,

    #[error("租户用户数已达到上限：当前 {current}，上限 {limit}（包含 owner 和停用用户）")]
    TenantUserLimitExceeded { current: i64, limit: i32 },

    #[error(
        "当前用户网关 Key 数已达到上限：当前 {current}，上限 {limit}（跨 Provider 合计，包含停用和失效 Key）"
    )]
    TenantGatewayKeyLimitExceeded { current: i64, limit: i32 },

    #[error("平台未允许本租户 owner 上传 WASM 插件")]
    TenantWasmUploadForbidden,
}

impl ConsoleError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::MissingDashboardToken => "missing_dashboard_token",
            Self::InvalidDashboardToken => "invalid_dashboard_token",
            Self::Forbidden => "forbidden",
            Self::TenantUserLimitExceeded { .. } => "tenant_user_limit_exceeded",
            Self::TenantGatewayKeyLimitExceeded { .. } => "tenant_gateway_key_limit_exceeded",
            Self::TenantWasmUploadForbidden => "tenant_wasm_upload_forbidden",
        }
    }
}
