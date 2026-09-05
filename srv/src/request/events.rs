//! 核心请求链路向后台 worker 发布的强类型事实。
//!
//! 本模块只定义事件契约和一个轻量发布端口，不包含任何 ClickHouse、PostgreSQL 或
//! worker 处理逻辑。发布使用有界队列的 `try_send`：队列满或关闭时允许丢失事件，只写
//! tracing 诊断，绝不等待后台容量，也绝不把失败反馈给模型请求链路。

use bytes::Bytes;
use chrono::{DateTime, Utc};
use tokio::sync::mpsc;
use tracing::{error, warn};
use uuid::Uuid;

/// provider 从调用方原始请求中提取的可选日志字段。
///
/// 这是请求生命周期的协议无关事实，因此放在事件契约层，由 provider 负责产生、后台
/// 日志投影负责消费，双方不需要互相依赖实现模块。
#[derive(Debug, Clone, Default)]
pub struct RequestLogFields {
    pub reasoning: Option<String>,
    pub service_tier: Option<String>,
    pub fast_mode: Option<bool>,
    /// GPT 请求检查成功后始终为 `Some`，分别表示压缩或普通请求；其他 provider 及尚未
    /// 完成协议检查的请求保持 `None`，避免把“未知”误写为普通请求。
    pub is_compaction: Option<bool>,
}

/// 一次真实上游模型调用产生的确定 token 用量。
#[derive(Debug, Clone, Copy, Default)]
pub struct TokenUsage {
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_output_tokens: i64,
    pub total_tokens: i64,
}

/// usage 对应的额度归属。
///
/// 该信息直接来自已经通过鉴权的网关请求，并随 usage 事件一同发送。额度 worker 不再
/// 通过请求日志聚合状态反查用户，避免两个后台消费者形成隐式依赖。
#[derive(Debug, Clone, Copy)]
pub struct UsageAttribution {
    pub user_id: Uuid,
    pub api_key_id: Uuid,
}

/// 网关 header 鉴权成功后立即产生的调用方归属。
///
/// 这些字段全部属于网关自身的 API Key、用户和 Provider 分组领域，不依赖请求体及
/// provider 私有协议。将其与请求检查结果分开发送后，即使调用方在上传 body 时断开，
/// ClickHouse 请求日志仍能保留已经确认的调用方归属。
#[derive(Debug)]
pub struct GatewayAuthDetails {
    pub tenant_id: String,
    pub api_key_id: Uuid,
    pub api_key_name: String,
    pub user_id: Uuid,
    pub username: String,
    pub provider_group_id: Uuid,
    pub provider_group_name: String,
}

/// provider 完成私有请求 DTO 检查后产生的协议字段。
///
/// `model` 的提取方式由 provider 协议决定，但模型白名单授权仍由通用网关负责；因此该
/// 事实在 inspect 成功后、模型授权前发布，未获授权的原始模型也能进入请求日志。
#[derive(Debug)]
pub struct RequestInspectionDetails {
    pub model: String,
    pub log_fields: RequestLogFields,
}

/// 流式 HTTP body 的真实传输结束原因。
///
/// Provider 的 completed/failed/error 等 SSE 事件只是协议内容，不决定 HTTP body 生命周期；
/// 只有底层上游 EOF、读取错误、空闲超时、响应插件错误或下游停止消费才会产生这里的结束事实。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamEndReason {
    UpstreamEof,
    UpstreamError,
    IdleTimeout,
    PluginError,
    DownstreamDisconnected,
}

impl StreamEndReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UpstreamEof => "upstream_eof",
            Self::UpstreamError => "upstream_error",
            Self::IdleTimeout => "stream_idle_timeout",
            Self::PluginError => "plugin_error",
            Self::DownstreamDisconnected => "downstream_disconnected",
        }
    }
}

/// 流结束前最后观察到的 provider 错误或通用传输错误。
///
/// 该事实与结束原因一起放入同一个终态事件，避免允许丢失的事件队列只收到结束信号、
/// 却丢失更早的错误事件，进而把失败请求误判成成功。
#[derive(Debug)]
pub struct StreamErrorRecord {
    pub kind: &'static str,
    pub body: String,
}

impl StreamErrorRecord {
    pub fn fluctuation() -> Self {
        Self {
            kind: "stream_fluctuation",
            body: "stream波动".to_owned(),
        }
    }
}

/// 请求链路已经观察到的唯一终态事实。
///
/// worker 收到本事件后一次性决定成功、异常或失败并结束聚合；请求链路若在发送终态前
/// 被取消或 panic，不再使用日志专用 RAII guard 兜底，只由 worker 超时回收。
#[derive(Debug)]
pub enum RequestEndResult {
    HttpSuccess,
    HttpFailure {
        status_code: u16,
        /// 与实际返回给调用方的响应复用同一批字节，不进行第二次序列化。
        body: Bytes,
    },
    Stream {
        reason: StreamEndReason,
        error: Option<StreamErrorRecord>,
    },
    RequestBodyInterrupted,
}

/// 请求生命周期中已经发生的事实。
///
/// 事件只携带 worker 完成后台投影所需的拥有型数据，不携带 `AppState`、流式 HTTP Body、
/// Redis lease 等核心运行对象。新增后台消费者可以解释这些事实，但不能反向控制请求。
#[derive(Debug)]
pub enum RequestEvent {
    Started {
        request_id: Uuid,
        provider: &'static str,
        route: &'static str,
        occurred_at: DateTime<Utc>,
    },
    GatewayAuthenticated {
        request_id: Uuid,
        details: GatewayAuthDetails,
    },
    RequestInspected {
        request_id: Uuid,
        details: RequestInspectionDetails,
    },
    /// 每次调度成功后发布，包括首次调用和重试；日志保留最后收到的资源 ID。
    ResourceSelected { request_id: Uuid, resource_id: Uuid },
    /// 最终下游响应第一次具备客户端可见正文的时间。
    ///
    /// Buffered 响应在完整正文已经确定、即将交给 Axum 时发布；streaming 响应在首个
    /// 非空 body chunk 真正从网关流中产出时发布。任何上游 attempt 的响应头都不能产生
    /// 本事件，避免重试失败响应污染客户端视角的首字耗时。
    ResponseStarted {
        request_id: Uuid,
        occurred_at: DateTime<Utc>,
    },
    UsageObserved {
        request_id: Uuid,
        attribution: UsageAttribution,
        usage: TokenUsage,
    },
    Ended {
        request_id: Uuid,
        occurred_at: DateTime<Utc>,
        result: RequestEndResult,
    },
}

impl RequestEvent {
    pub(crate) fn request_id(&self) -> Uuid {
        match self {
            Self::Started { request_id, .. }
            | Self::GatewayAuthenticated { request_id, .. }
            | Self::RequestInspected { request_id, .. }
            | Self::ResourceSelected { request_id, .. }
            | Self::ResponseStarted { request_id, .. }
            | Self::UsageObserved { request_id, .. }
            | Self::Ended { request_id, .. } => *request_id,
        }
    }

    pub(crate) const fn kind(&self) -> &'static str {
        match self {
            Self::Started { .. } => "request_started",
            Self::GatewayAuthenticated { .. } => "gateway_authenticated",
            Self::RequestInspected { .. } => "request_inspected",
            Self::ResourceSelected { .. } => "resource_selected",
            Self::ResponseStarted { .. } => "response_started",
            Self::UsageObserved { .. } => "usage_observed",
            Self::Ended { .. } => "request_ended",
        }
    }
}

/// 核心请求链路持有的唯一后台事件发布端口。
#[derive(Clone)]
pub struct RequestEventPublisher {
    tx: mpsc::Sender<RequestEvent>,
}

impl RequestEventPublisher {
    pub(crate) fn channel(capacity: usize) -> (Self, mpsc::Receiver<RequestEvent>) {
        let (tx, rx) = mpsc::channel(capacity);
        (Self { tx }, rx)
    }

    /// 尝试立即发布事件；无论成功、队列已满还是 worker 已退出，都不会等待或返回错误。
    pub fn emit(&self, event: RequestEvent) {
        let request_id = event.request_id();
        let event_kind = event.kind();
        match self.tx.try_send(event) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => warn!(
                request_id = %request_id,
                event_kind,
                "后台请求事件队列已满，当前事件已丢弃"
            ),
            Err(mpsc::error::TrySendError::Closed(_)) => error!(
                request_id = %request_id,
                event_kind,
                "后台请求事件队列已关闭，当前事件已丢弃"
            ),
        }
    }
}
