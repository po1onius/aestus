use thiserror::Error;
use uuid::Uuid;

mod console;
pub use console::ConsoleError;

pub type AppResult<T> = Result<T, AppError>;

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

    #[error("HTTP client 初始化失败: {message}")]
    HttpClientBuild { message: String },

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

    #[error("用户在 {provider} provider 上的并发请求已达到上限: current={current}, limit={limit}")]
    UserConcurrencyExceeded {
        provider: String,
        current: u32,
        limit: u32,
    },

    #[error("API Key 无权调用模型: {model}")]
    ModelNotAllowed { model: String },

    #[error("API Key 分组属于 {key_provider}，不能调用 {requested_provider} 接口")]
    GatewayKeyProviderMismatch {
        key_provider: String,
        requested_provider: String,
    },

    #[error("API Key 所属 Provider 分组已归档")]
    GatewayKeyGroupUnavailable,

    #[error(transparent)]
    Console(#[from] ConsoleError),

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

impl AppError {
    pub fn code(&self) -> &'static str {
        match self {
            AppError::Console(error) => error.code(),
            AppError::ReadConfig { .. } => "read_config_failed",
            AppError::InvalidConfig { .. } => "invalid_config",
            AppError::MissingConfig { .. } => "missing_config",
            AppError::Startup { .. } => "startup_failed",
            AppError::HttpClientBuild { .. } => "http_client_build_failed",
            AppError::DbPoolBuild { .. } => "db_pool_build_failed",
            AppError::DbPoolGet { .. } => "db_pool_get_failed",
            AppError::DbQuery { .. } => "db_query_failed",
            AppError::RedisClient { .. } => "redis_client_failed",
            AppError::Redis { .. } => "redis_failed",
            AppError::MissingApiKey => "missing_api_key",
            AppError::InvalidApiKey => "invalid_api_key",
            AppError::DisabledApiKey => "disabled_api_key",
            AppError::UserQuotaExceeded => "user_quota_exceeded",
            AppError::UserConcurrencyExceeded { .. } => "user_concurrency_exceeded",
            AppError::ModelNotAllowed { .. } => "model_not_allowed",
            AppError::GatewayKeyProviderMismatch { .. } => "gateway_key_provider_mismatch",
            AppError::GatewayKeyGroupUnavailable => "gateway_key_group_unavailable",
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
}

impl From<diesel::result::Error> for AppError {
    fn from(source: diesel::result::Error) -> Self {
        Self::DbQuery {
            message: source.to_string(),
        }
    }
}
