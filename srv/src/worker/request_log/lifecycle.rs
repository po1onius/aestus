use std::collections::HashMap;

use bytes::Bytes;
use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use tracing::{info, warn};
use uuid::Uuid;

use crate::request::events::{
    GatewayAuthDetails as GatewayAuthDetailsEvent, RequestEndResult, RequestEvent,
    RequestInspectionDetails as RequestInspectionDetailsEvent, StreamEndReason, StreamErrorRecord,
    TokenUsage,
};

use super::writer::RequestLogWriter;

/// 最长保留一天未收到完成事件的聚合状态。
///
/// 核心链路允许在队列满时丢弃事件，因此后台必须自行回收缺少原子终态事件的条目，避免
/// 非核心日志功能因部分事件丢失而持续占用内存。超时条目仍写入明确终止原因供排障。
const REQUEST_LOG_STALE_AFTER_HOURS: i64 = 24;

/// 网关鉴权成功后确定的调用方归属。
#[derive(Debug)]
pub(super) struct GatewayAttribution {
    pub(super) tenant_id: Uuid,
    /// 请求发生时的 Key 名称快照，用于请求日志持久化和 Dashboard 展示。
    pub(super) api_key_name: String,
    pub(super) user_id: Uuid,
    /// 请求发生时的不可变用户名快照，ClickHouse 展示不依赖后续 PostgreSQL 回查。
    pub(super) username: String,
    pub(super) provider_group_id: Uuid,
    pub(super) provider_group_name: String,
}

/// provider 完成私有 DTO 检查后确定的协议字段。
#[derive(Debug)]
pub(super) struct RequestInspection {
    pub(super) model: String,
    pub(super) reasoning: Option<String>,
    pub(super) service_tier: Option<String>,
    pub(super) fast_mode: Option<bool>,
    pub(super) is_compaction: Option<bool>,
}

/// 请求结束时可供诊断的错误信息。
///
/// HTTP 保存与调用方实际收到的响应完全相同的状态码和正文；流式请求保存 provider
/// 错误事件或通用传输异常诊断，二者共用稳定 JSON 形状。
#[derive(Debug, Serialize)]
pub(super) struct RequestErrorResponse {
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    status_code: Option<u16>,
    body: String,
}

impl RequestErrorResponse {
    fn http(status_code: u16, body: Bytes) -> Self {
        Self {
            kind: "http".to_owned(),
            status_code: Some(status_code),
            body: String::from_utf8_lossy(&body).to_string(),
        }
    }

    fn stream(error: StreamErrorRecord) -> Self {
        Self {
            kind: error.kind.to_owned(),
            status_code: None,
            body: error.body,
        }
    }
}

#[derive(Debug, Serialize)]
pub(super) struct RequestTermination {
    kind: &'static str,
}

/// 请求日志在收到唯一终态事件时确定的稳定状态。
///
/// 状态是 ClickHouse 的一级筛选维度；`extra` 只保存具体错误正文和异常结束原因，前端
/// 不再解析半结构化 JSON 反推请求是否成功。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RequestLogStatus {
    Success,
    Abnormal,
    Failed,
}

impl RequestLogStatus {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Abnormal => "abnormal",
            Self::Failed => "failed",
        }
    }
}

/// ClickHouse `extra` 的强类型内存表示。
///
/// 半结构化 JSON 只在最终序列化边界生成，不向业务模块开放任意字段 merge，从而避免
/// 不同调用点静默覆盖同名 key。
#[derive(Debug, Default, Serialize)]
pub(super) struct RequestLogExtra {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) error_response: Option<RequestErrorResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) lifecycle_termination: Option<RequestTermination>,
}

/// 单条请求日志在后台事件消费期间的聚合状态。
#[derive(Debug)]
pub(super) struct RequestLogEntry {
    pub(super) request_id: Uuid,
    pub(super) provider: String,
    pub(super) route: String,
    pub(super) gateway_attribution: Option<GatewayAttribution>,
    pub(super) inspection: Option<RequestInspection>,
    pub(super) request_started_at: DateTime<Utc>,
    pub(super) response_started_at: Option<DateTime<Utc>>,
    pub(super) response_finished_at: Option<DateTime<Utc>>,
    pub(super) token_usage: Option<TokenUsage>,
    pub(super) extra: RequestLogExtra,
}

/// 只在 worker 完成终态判定后产生并投递给 ClickHouse writer 的不可变快照。
///
/// 将 `status` 与进行中的 `RequestLogEntry` 分离，避免尚未结束的请求被预置为成功，也
/// 从类型上保证 writer 收到的每条日志都已经完成成功、异常或失败判定。
pub(super) struct FinalizedRequestLogEntry {
    pub(super) entry: RequestLogEntry,
    pub(super) status: RequestLogStatus,
}

impl RequestLogEntry {
    fn new(
        request_id: Uuid,
        provider: &'static str,
        route: &'static str,
        request_started_at: DateTime<Utc>,
    ) -> Self {
        Self {
            request_id,
            provider: provider.to_owned(),
            route: route.to_owned(),
            gateway_attribution: None,
            inspection: None,
            request_started_at,
            response_started_at: None,
            response_finished_at: None,
            token_usage: None,
            extra: RequestLogExtra::default(),
        }
    }
}

/// 完全运行在 worker 任务中的请求日志投影器。
///
/// 单个事件消费循环按接收顺序调用本类型，因此普通 `HashMap` 已足够，不需要请求热路径
/// 共享锁或 `DashMap`。事件缺失只影响日志完整性，不会回调或阻塞核心请求。
pub(super) struct RequestLogLifecycle {
    entries: HashMap<Uuid, RequestLogEntry>,
    writer: RequestLogWriter,
}

impl RequestLogLifecycle {
    pub(super) fn new(writer: RequestLogWriter) -> Self {
        Self {
            entries: HashMap::new(),
            writer,
        }
    }

    pub(super) fn handle(&mut self, event: RequestEvent) {
        match event {
            RequestEvent::Started {
                request_id,
                provider,
                route,
                occurred_at,
            } => self.start(request_id, provider, route, occurred_at),
            RequestEvent::GatewayAuthenticated {
                request_id,
                details,
            } => self.set_gateway_attribution(request_id, details),
            RequestEvent::RequestInspected {
                request_id,
                details,
            } => self.set_request_inspection(request_id, details),
            RequestEvent::ResponseStarted {
                request_id,
                occurred_at,
            } => self.mark_response_started(request_id, occurred_at),
            RequestEvent::UsageObserved {
                request_id, usage, ..
            } => self.record_token_usage(request_id, usage),
            RequestEvent::Ended {
                request_id,
                occurred_at,
                result,
            } => self.finish(request_id, occurred_at, result),
        }
    }

    fn start(
        &mut self,
        request_id: Uuid,
        provider: &'static str,
        route: &'static str,
        request_started_at: DateTime<Utc>,
    ) {
        let replaced = self
            .entries
            .insert(
                request_id,
                RequestLogEntry::new(request_id, provider, route, request_started_at),
            )
            .is_some();
        if replaced {
            warn!(request_id = %request_id, provider, route, "请求日志 request ID 重复，旧聚合状态已被替换");
        }
        info!(request_id = %request_id, provider, route, "worker 已创建请求日志聚合上下文");
    }

    fn set_gateway_attribution(&mut self, request_id: Uuid, details: GatewayAuthDetailsEvent) {
        let GatewayAuthDetailsEvent {
            tenant_id,
            api_key_id,
            api_key_name,
            user_id,
            username,
            provider_group_id,
            provider_group_name,
        } = details;
        let Some(entry) = self.entries.get_mut(&request_id) else {
            warn!(
                request_id = %request_id,
                api_key_id = %api_key_id,
                user_id = %user_id,
                provider_group_id = %provider_group_id,
                "worker 收到网关鉴权事件时未命中日志聚合上下文"
            );
            return;
        };

        if entry.gateway_attribution.is_some() {
            warn!(request_id = %request_id, "worker 收到重复网关鉴权事件，使用最新归属快照");
        }
        entry.gateway_attribution = Some(GatewayAttribution {
            tenant_id,
            api_key_name,
            user_id,
            username,
            provider_group_id,
            provider_group_name,
        });
        info!(
            request_id = %request_id,
            tenant_id = %tenant_id,
            api_key_id = %api_key_id,
            user_id = %user_id,
            provider_group_id = %provider_group_id,
            "worker 已合并网关 API Key、用户和分组归属"
        );
    }

    fn set_request_inspection(&mut self, request_id: Uuid, details: RequestInspectionDetailsEvent) {
        let RequestInspectionDetailsEvent { model, log_fields } = details;
        let Some(entry) = self.entries.get_mut(&request_id) else {
            warn!(
                request_id = %request_id,
                requested_model = %model,
                "worker 收到 provider 请求检查事件时未命中日志聚合上下文"
            );
            return;
        };

        if entry.inspection.is_some() {
            warn!(request_id = %request_id, "worker 收到重复 provider 请求检查事件，使用最新协议字段");
        }
        info!(
            request_id = %request_id,
            requested_model = %model,
            "worker 已合并 provider 请求检查字段"
        );
        entry.inspection = Some(RequestInspection {
            model,
            reasoning: log_fields.reasoning,
            service_tier: log_fields.service_tier,
            fast_mode: log_fields.fast_mode,
            is_compaction: log_fields.is_compaction,
        });
    }

    fn mark_response_started(&mut self, request_id: Uuid, occurred_at: DateTime<Utc>) {
        let Some(entry) = self.entries.get_mut(&request_id) else {
            warn!(request_id = %request_id, "worker 收到响应开始事件时未命中日志聚合上下文");
            return;
        };
        if entry.response_started_at.is_some() {
            warn!(request_id = %request_id, "worker 收到重复下游响应首字事件，保留首次时间");
            return;
        }
        entry.response_started_at = Some(occurred_at);
        info!(
            request_id = %request_id,
            response_started_at = %occurred_at,
            "worker 已记录客户端视角的下游响应首字时间"
        );
    }

    fn record_token_usage(&mut self, request_id: Uuid, usage: TokenUsage) {
        let Some(entry) = self.entries.get_mut(&request_id) else {
            warn!(
                request_id = %request_id,
                total_tokens = usage.total_tokens,
                "worker 收到 token usage 事件时未命中日志聚合上下文"
            );
            return;
        };

        entry.token_usage = Some(usage);
        info!(
            request_id = %request_id,
            input_tokens = usage.input_tokens,
            cached_input_tokens = usage.cached_input_tokens,
            output_tokens = usage.output_tokens,
            reasoning_output_tokens = usage.reasoning_output_tokens,
            total_tokens = usage.total_tokens,
            "worker 已把 token usage 合并进请求日志"
        );
    }

    fn finish(
        &mut self,
        request_id: Uuid,
        response_finished_at: DateTime<Utc>,
        result: RequestEndResult,
    ) {
        let (status, error_response, termination) = match result {
            RequestEndResult::HttpSuccess => (RequestLogStatus::Success, None, None),
            RequestEndResult::HttpFailure { status_code, body } => (
                RequestLogStatus::Failed,
                Some(RequestErrorResponse::http(status_code, body)),
                None,
            ),
            RequestEndResult::Stream { reason, error } => {
                let error_response = error.map(RequestErrorResponse::stream);
                match reason {
                    // EOF 是 HTTP body 的正常结束。此前观察到 provider failed/error 时，
                    // 请求有明确失败结果；否则即使没有应用层 terminal event 也属于成功。
                    StreamEndReason::UpstreamEof if error_response.is_some() => {
                        (RequestLogStatus::Failed, error_response, None)
                    }
                    StreamEndReason::UpstreamEof => (RequestLogStatus::Success, None, None),
                    // 传输错误、空闲超时和下游取消都没有正常完成 HTTP body，因此异常
                    // 优先于此前可能已经观察到的 provider 失败，错误正文仍保留供诊断。
                    StreamEndReason::UpstreamError
                    | StreamEndReason::IdleTimeout
                    | StreamEndReason::PluginError
                    | StreamEndReason::DownstreamDisconnected => (
                        RequestLogStatus::Abnormal,
                        error_response,
                        Some(reason.as_str()),
                    ),
                }
            }
            RequestEndResult::RequestBodyInterrupted => (
                RequestLogStatus::Abnormal,
                None,
                Some("request_body_interrupted"),
            ),
        };
        self.finalize(
            request_id,
            response_finished_at,
            status,
            error_response,
            termination,
        );
    }

    fn finalize(
        &mut self,
        request_id: Uuid,
        response_finished_at: DateTime<Utc>,
        status: RequestLogStatus,
        error_response: Option<RequestErrorResponse>,
        termination: Option<&'static str>,
    ) {
        let Some(mut entry) = self.entries.remove(&request_id) else {
            warn!(request_id = %request_id, "worker 收到请求完成事件时未命中日志聚合上下文");
            return;
        };

        entry.response_finished_at = Some(response_finished_at);
        entry.extra.error_response = error_response;
        entry.extra.lifecycle_termination = termination.map(|kind| RequestTermination { kind });
        info!(
            request_id = %request_id,
            request_status = status.as_str(),
            termination = termination.unwrap_or("<none>"),
            error_response_present = entry.extra.error_response.is_some(),
            "worker 已根据请求终态完成日志状态判定"
        );
        self.writer
            .submit(FinalizedRequestLogEntry { entry, status });
    }

    /// 回收因允许丢失完成事件而残留的聚合条目。
    pub(super) fn evict_stale_entries(&mut self, now: DateTime<Utc>) {
        let stale_before = now - Duration::hours(REQUEST_LOG_STALE_AFTER_HOURS);
        let stale_request_ids = self
            .entries
            .iter()
            .filter_map(|(request_id, entry)| {
                (entry.request_started_at <= stale_before).then_some(*request_id)
            })
            .collect::<Vec<_>>();

        for request_id in stale_request_ids {
            warn!(
                request_id = %request_id,
                stale_after_hours = REQUEST_LOG_STALE_AFTER_HOURS,
                "worker 请求日志聚合上下文长时间未收到完成事件，执行超时收尾"
            );
            self.finalize(
                request_id,
                now,
                RequestLogStatus::Abnormal,
                None,
                Some("request_event_timeout"),
            );
        }
    }
}
