use std::{collections::VecDeque, future::Future};

use axum::{
    body::{Body, Bytes},
    http::{HeaderMap, Response, StatusCode, Uri},
};
use chrono::{DateTime, Utc};
use futures_util::stream::BoxStream;

use crate::{
    config::AppConfig,
    err::{AppError, AppResult},
    infra::http_client::HttpClientProfile,
    provider::{
        maintenance::MaintenanceProvider,
        resource::{UpstreamResource, UpstreamResourceKind},
    },
    request_body::CachedBody,
};

pub use crate::request_event::{RequestLogFields, StreamErrorRecord, TokenUsage};

/// 单个完整 SSE item（包含字段、数据和结尾空行）的统一大小上限。
///
/// Responses 中的图片等合法结果可能远大于传统文本事件，因此这里与 sub2api 默认的
/// `gateway.max_line_size` 保持一致，允许最多 500 MiB。Provider 旁路观察器、插件拼包
/// 缓冲以及插件 ABI 输入/输出必须共用该值，避免同一响应在前后处理阶段受到不同限制。
pub const MAX_SSE_ITEM_BYTES: usize = 500 * 1024 * 1024;

/// provider 对调用方原始请求执行一次协议检查后，交给通用 pipeline 的统一结果。
///
/// provider 内部可以使用私有强类型 DTO 完成 JSON 解析、必填字段校验和协议归一化；通用层
/// 只消费模型授权、调度粘性与日志所需结果，不持有也不感知 provider 私有解析类型。
pub struct RequestInspection {
    pub requested_model: String,
    pub sticky_key: Option<String>,
    pub log_fields: RequestLogFields,
}

/// provider gateway 可以暴露给模型调用方的协议无关错误类别。
///
/// 这里刻意不复用 Dashboard 的 `AppError::IntoResponse`：Dashboard 与模型协议面对的是
/// 不同调用方，也有不同的 wire shape。通用 gateway 只在这一层决定哪些信息可以公开，
/// GPT、Claude adapter 随后只能负责协议编码，不能再读取包含内部诊断的 `AppError`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderVisibleErrorKind {
    Authentication,
    Permission,
    InvalidRequest,
    RateLimit,
    Gateway,
}

/// 经过 gateway 公共边界脱敏、可安全返回给模型调用方的错误投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderVisibleError {
    pub status: StatusCode,
    pub kind: ProviderVisibleErrorKind,
    /// 大多数错误使用网关定义的稳定 code；请求插件主动拒绝时保留插件公开 code，方便
    /// SDK 和调用方精确定位可修正的协议字段，因此这里不能再限制为静态字符串。
    pub code: String,
    pub message: String,
}

impl ProviderVisibleError {
    /// 将内部 `AppError` 投影为 provider 可见错误。
    ///
    /// 鉴权、授权、额度和请求格式错误需要保留可操作信息，否则调用方无法修正请求；数据库、
    /// Redis、缓存和普通上游技术故障折叠为稳定的 `gateway_error`，资源不可用或可重试失败
    /// 耗尽则使用 `resource_error`。两者都不泄露内部诊断，并保留合理的 500/502/503 status
    /// 便于 SDK 判断是否应该重试；原始技术错误只写 tracing。
    pub fn from_app_error(error: &AppError) -> Self {
        let (kind, code) = match error {
            AppError::MissingApiKey | AppError::InvalidApiKey | AppError::DisabledApiKey => {
                (ProviderVisibleErrorKind::Authentication, "invalid_api_key")
            }
            AppError::ModelNotAllowed { .. } => {
                (ProviderVisibleErrorKind::Permission, "model_not_allowed")
            }
            AppError::GatewayKeyProviderMismatch { .. } => (
                ProviderVisibleErrorKind::Permission,
                "gateway_key_provider_mismatch",
            ),
            AppError::GatewayKeyGroupUnavailable => (
                ProviderVisibleErrorKind::Permission,
                "gateway_key_group_unavailable",
            ),
            AppError::UserQuotaExceeded => {
                (ProviderVisibleErrorKind::RateLimit, "user_quota_exceeded")
            }
            AppError::BadRequest { .. } => (
                ProviderVisibleErrorKind::InvalidRequest,
                "invalid_request_error",
            ),
            AppError::PluginRequestRejected { code, .. } => {
                (ProviderVisibleErrorKind::InvalidRequest, code.as_str())
            }
            AppError::PayloadTooLarge { .. } => (
                ProviderVisibleErrorKind::InvalidRequest,
                "request_too_large",
            ),
            // gateway 会在 provider 编码前截获此错误，并以空 499 收尾。这里仍提供一个
            // 安全的防御性投影，避免未来新增调用路径时又退化为含糊的 gateway_error。
            AppError::RequestBodyInterrupted { .. } => (
                ProviderVisibleErrorKind::InvalidRequest,
                "request_body_interrupted",
            ),
            AppError::ReadConfig { .. }
            | AppError::InvalidConfig { .. }
            | AppError::MissingConfig { .. }
            | AppError::Startup { .. }
            | AppError::DbPoolBuild { .. }
            | AppError::DbPoolGet { .. }
            | AppError::DbQuery { .. }
            | AppError::RedisClient { .. }
            | AppError::Redis { .. }
            // Dashboard 鉴权错误属于另一套响应边界。它们正常情况下不可能进入 provider
            // gateway；若未来错误接线，也按内部故障处理，绝不伪装成模型 API Key 错误。
            | AppError::MissingDashboardToken
            | AppError::InvalidDashboardToken
            | AppError::Forbidden
            | AppError::BodyCache { .. }
            | AppError::Plugin { .. }
            | AppError::ProviderUpstream { .. }
            | AppError::Email { .. }
            | AppError::ProviderStateSyncFailed { .. } => {
                (ProviderVisibleErrorKind::Gateway, "gateway_error")
            }
            // 调用方请求本身没有错误，但网关资源池已经无法继续完成请求。它仍使用
            // provider 的网关错误 wire 外壳，只把稳定公共 code 区分为 resource_error。
            AppError::ResourceError { .. } => {
                (ProviderVisibleErrorKind::Gateway, "resource_error")
            }
        };
        let status = if matches!(
            error,
            AppError::MissingDashboardToken | AppError::InvalidDashboardToken | AppError::Forbidden
        ) {
            StatusCode::INTERNAL_SERVER_ERROR
        } else {
            error.status_code()
        };
        let message = match error {
            AppError::RequestBodyInterrupted { .. } => "request body interrupted".to_owned(),
            AppError::PluginRequestRejected { message, .. } => message.clone(),
            _ if kind == ProviderVisibleErrorKind::Gateway => "gateway error".to_owned(),
            _ => error.to_string(),
        };

        Self {
            status,
            kind,
            code: code.to_owned(),
            message,
        }
    }
}

/// provider 已完成 wire 编码的最终错误响应。
///
/// `body` 是唯一真源：gateway 会先把同一批字节写入请求生命周期，再原样交给 HTTP
/// response，避免 ClickHouse 与调用方实际收到的错误因二次序列化而发生偏差。
pub struct EncodedProviderError {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Bytes,
}

impl EncodedProviderError {
    pub fn into_response(self) -> Response<Body> {
        let mut response = Response::new(Body::from(self.body));
        *response.status_mut() = self.status;
        *response.headers_mut() = self.headers;
        response
    }
}

/// provider 从上游协议中识别出的中立事实。
///
/// 该类型不描述刷新 token、探活 API Key 或写入 quota 等维护动作；具体状态迁移由
/// maintenance 根据 provider、资源类型和运行配置决定。
#[derive(Debug, Clone)]
pub enum UpstreamFeedback {
    /// 官方 API Key 只保留这一种未分类资源错误；完整原始正文已经由 protocol tracing 记录。
    Error {
        reason: String,
    },
    AuthenticationRejected {
        reason: String,
    },
    RateLimited {
        resets_at: Option<DateTime<Utc>>,
        reason: String,
    },
    QuotaExhausted {
        resets_at: Option<DateTime<Utc>>,
        reason: String,
    },
    TemporarilyUnavailable {
        reason: String,
    },
    EntitlementMissing {
        reason: String,
    },
}

impl UpstreamFeedback {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Error { .. } => "error",
            Self::AuthenticationRejected { .. } => "authentication_rejected",
            Self::RateLimited { .. } => "rate_limited",
            Self::QuotaExhausted { .. } => "quota_exhausted",
            Self::TemporarilyUnavailable { .. } => "temporarily_unavailable",
            Self::EntitlementMissing { .. } => "entitlement_missing",
        }
    }

    /// API Key maintenance 不区分错误种类，只保留 provider 已经安全归一化的诊断文本。
    pub fn into_reason(self) -> String {
        match self {
            Self::Error { reason }
            | Self::AuthenticationRejected { reason }
            | Self::RateLimited { reason, .. }
            | Self::QuotaExhausted { reason, .. }
            | Self::TemporarilyUnavailable { reason }
            | Self::EntitlementMissing { reason } => reason,
        }
    }
}

/// 可供每次上游 attempt 重放的调用方原始请求。
pub struct ReplayableRequest {
    pub request_id: uuid::Uuid,
    pub uri: Uri,
    pub headers: HeaderMap,
    pub body: CachedBody,
}

/// provider 构造出的上游请求草稿。
pub struct UpstreamRequestDraft {
    /// 声明本次请求所需的 client 能力；通用 executor 只负责按 profile 选择连接池。
    pub client_profile: HttpClientProfile,
    pub method: reqwest::Method,
    pub url: String,
    pub headers: HeaderMap,
    pub body: UpstreamRequestBodyMode,
}

/// 上游 URL、method 与 HTTP client profile 不属于 header/body 改造，始终由宿主控制。
/// 插件模式和原生模式共用这一目标描述，避免插件获得任意网络访问或改写目标地址的能力。
pub struct UpstreamRequestTarget {
    pub client_profile: HttpClientProfile,
    pub method: reqwest::Method,
    pub url: String,
}

/// provider 对本次上游 attempt 请求体来源及物化需求的声明。
///
/// 请求缓存可能位于临时文件中，因此“是否需要完整字节”必须由请求草稿明确表达，而不是
/// 通过另一个 trait predicate 与最终化动作松散配对。通用 pipeline 会先按该声明和 body
/// override 决定是否读取完整字节，再把可选的 mutable body 交给唯一的请求最终化 hook。
pub enum UpstreamRequestBodyMode {
    /// 没有 body override 时直接从不可变缓存重放；provider 最终化只处理 header。
    ReplayOriginal,
    /// 即使没有 body override，也读取原始完整字节供 provider 最终化。
    MaterializeOriginal,
}

/// 一次真实上游 HTTP attempt 的 tracing 关联上下文。
///
/// 同一个 gateway `request_id` 可以经历多次重试；因此日志必须同时包含 attempt 序号和
/// 当时选中的资源 revision，管理员才能把完整上游错误正文还原到具体网络请求。
#[derive(Debug, Clone, Copy)]
pub struct UpstreamAttemptContext {
    pub request_id: uuid::Uuid,
    pub provider: &'static str,
    pub resource_kind: UpstreamResourceKind,
    pub resource_id: uuid::Uuid,
    pub runtime_revision: i64,
    pub attempt_number: usize,
    pub max_attempts: usize,
}

/// provider 对 buffered 上游响应的纯协议分类结果。
pub struct BufferedProtocolResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Bytes,
    pub record_error_response: bool,
    pub retry: bool,
    /// 仅当 provider 能确认“当前请求不应再次使用同一资源、但该事实不应改变资源全局状态”
    /// 时设置。通用 executor 只在确实还有下一次 attempt 时把当前资源加入请求级排除集合。
    pub exclude_resource_on_retry: bool,
    pub feedback: Option<UpstreamFeedback>,
    pub usage: Option<TokenUsage>,
}

/// provider 流观察器处理一批上游字节后的结果。
#[derive(Default)]
pub struct StreamUpdate {
    pub output: VecDeque<Bytes>,
    pub feedback: Option<UpstreamFeedback>,
}

/// 上游流正常结束或被中断时，provider 交给 pipeline 的最终解析结果。
#[derive(Default)]
pub struct StreamCompletion {
    pub output: VecDeque<Bytes>,
    pub feedback: Option<UpstreamFeedback>,
    pub usage: Option<TokenUsage>,
    pub error: Option<StreamErrorRecord>,
}

/// 只负责 provider 流式协议解析与必要的 wire 字节改写。
///
/// 实现不能调度资源、提交 maintenance、扣减额度或结束请求生命周期。
pub trait StreamObserver: Send + Unpin + 'static {
    fn observe(&mut self, chunk: Bytes) -> StreamUpdate;

    fn complete(&mut self) -> StreamCompletion;
}

/// provider 已完成响应头分类后交给通用 pipeline 的流式响应。
pub struct StreamingProtocolResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub stream: BoxStream<'static, Result<Bytes, reqwest::Error>>,
    /// 插件完全接管 SSE item 时为 `None`；空 stream 插槽继续使用 provider 原生 observer。
    pub observer: Option<Box<dyn StreamObserver>>,
}

pub enum ProtocolResponse {
    Buffered(BufferedProtocolResponse),
    Streaming(StreamingProtocolResponse),
}

/// provider 在读取或解析响应时无法产出可返回响应。
///
/// 网络接收失败与 provider 本地响应构造失败具有完全不同的生命周期语义：前者由通用
/// executor 固定重试且不产生资源回执，后者说明 adapter 无法构造合法下游响应，不能通过
/// 换一个凭证解决。使用封闭枚举把这项策略编码在通用协议层，避免各 provider 分别设置
/// `retry` 或误附带资源回执。
pub struct ProtocolFailure {
    error: AppError,
    kind: ProtocolFailureKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProtocolFailureKind {
    Network,
    Adapter,
}

impl ProtocolFailure {
    fn network(error: AppError) -> Self {
        Self {
            error,
            kind: ProtocolFailureKind::Network,
        }
    }

    pub fn adapter(error: AppError) -> Self {
        Self {
            error,
            kind: ProtocolFailureKind::Adapter,
        }
    }

    pub fn is_network(&self) -> bool {
        self.kind == ProtocolFailureKind::Network
    }

    pub fn error(&self) -> &AppError {
        &self.error
    }

    pub fn into_error(self) -> AppError {
        self.error
    }
}

/// 完整读取一个需要 buffered 处理的上游响应正文。
///
/// HTTP status 和 provider 私有错误正文仍由 adapter 分类；正文传输本身则是所有 provider
/// 共用的网络能力。超时或 reqwest 读取错误统一产生 `Network` failure，executor 会从原始
/// 请求重放下一次 attempt，且不会把公共网络波动归因到账号或官方 API Key。
pub async fn read_buffered_upstream_body(
    config: &AppConfig,
    provider: &'static str,
    response: reqwest::Response,
) -> Result<Bytes, ProtocolFailure> {
    let timeout_seconds = config.provider_upstream_timeout_seconds.max(1);
    tokio::time::timeout(
        std::time::Duration::from_secs(timeout_seconds),
        response.bytes(),
    )
    .await
    .map_err(|_| {
        ProtocolFailure::network(AppError::ProviderUpstream {
            provider: provider.to_owned(),
            message: format!("读取上游 buffered 响应超时: {timeout_seconds} 秒"),
        })
    })?
    .map_err(|source| {
        ProtocolFailure::network(AppError::ProviderUpstream {
            provider: provider.to_owned(),
            message: format!("读取上游 buffered 响应失败: {source}"),
        })
    })
}

/// provider HTTP 协议适配边界。
///
/// 实现方法只解析和构造 header/request/response，不执行 scheduler、maintenance、用户额度
/// 或请求生命周期操作；关联类型仅在编译期声明该协议适配器对应的维护策略。
pub trait ProviderProtocol: Send + Sync + 'static {
    /// 将 operation 协议适配器与对应 maintenance 实现在类型层绑定。
    ///
    /// provider 名称统一取自 maintenance，调用方不再分别传入两个可能不匹配的泛型参数。
    type Maintenance: MaintenanceProvider;

    fn provider_name() -> &'static str {
        Self::Maintenance::NAME
    }

    /// 一次解析并提取通用 pipeline 需要的全部请求信息。
    ///
    /// 原始 body 仍由请求缓存负责透传和重放；该方法不得修改请求体，也不要把 provider
    /// 私有 DTO 暴露给通用层。
    fn inspect_request(body: &[u8]) -> AppResult<RequestInspection>;

    /// 将已经脱敏的公共错误编码为 provider 原生 wire shape。
    ///
    /// 实现只能消费 `ProviderVisibleError`，从类型层阻止 provider adapter 意外把内部
    /// `AppError` 的数据库、Redis、URL 或上游传输诊断写回调用方。
    fn encode_error(error: &ProviderVisibleError, request_id: uuid::Uuid) -> EncodedProviderError;

    /// 只确定当前资源对应的上游网络目标，不读取、筛选或改写原始 header/body。
    fn prepare_upstream_target(
        config: &AppConfig,
        resource: &UpstreamResource,
        request: &ReplayableRequest,
    ) -> AppResult<UpstreamRequestTarget>;

    fn prepare_upstream_request(
        config: &AppConfig,
        resource: &UpstreamResource,
        request: &ReplayableRequest,
    ) -> AppResult<UpstreamRequestDraft>;

    /// 在通用 header/body override 之后，一次性完成 provider 私有的上游请求最终化。
    ///
    /// `body` 只有在草稿要求物化，或通用 body override 必须应用时才为 `Some`；
    /// header-only provider 不应迫使临时文件请求重新读入内存。实现必须在这里最终注入
    /// 真实凭证，从而覆盖调用方或管理员 override 中可能存在的认证 header；也可同时
    /// 替换完整 body，或写入与实际凭证绑定的 attribution。本 hook 不决定或新增重试策略。
    fn finalize_upstream_request(
        resource: &UpstreamResource,
        request_id: uuid::Uuid,
        headers: &mut HeaderMap,
        body: Option<&mut Bytes>,
    ) -> AppResult<()>;

    fn handle_response<'a>(
        config: &'a AppConfig,
        resource: &'a UpstreamResource,
        attempt: UpstreamAttemptContext,
        response: reqwest::Response,
    ) -> impl Future<Output = Result<ProtocolResponse, ProtocolFailure>> + Send + 'a;
}
