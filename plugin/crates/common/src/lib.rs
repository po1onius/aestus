//! 三个 GPT 插件组件共享的纯转换逻辑。
//!
//! 这里不依赖任何一份 WIT 生成类型，避免 request、buffered response、stream
//! response 三个 world 之间互相耦合。各组件入口只负责 ABI 类型转换，字段语义、
//! header 安全规则、usage/maintenance 判定都集中在本 crate，确保流式与非流式行为一致。

pub mod functions;
pub mod headers;
pub mod request;
pub mod response;
pub mod responses_sse;
pub mod sse;

/// 与 WIT `header` 等价、但不依赖具体 world 的内部类型。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub name: String,
    pub value: Vec<u8>,
}

/// 宿主要求的累计 token 快照。`total_tokens` 始终由 input + output 计算，避免把
/// 上游偶尔缺失或不一致的 total 值传给宿主后触发 ABI 校验错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Usage {
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_output_tokens: i64,
    pub total_tokens: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LimitFeedback {
    pub resets_at_unix_seconds: Option<i64>,
    pub reason: String,
}

/// maintenance 回执的协议无关内部表示。响应插件只根据原始上游状态和错误对象生成
/// 一次回执；宿主消费 effects，不再扫描插件改造后的下游 body。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Feedback {
    Error(String),
    AuthenticationRejected(String),
    RateLimited(LimitFeedback),
    QuotaExhausted(LimitFeedback),
    TemporarilyUnavailable(String),
    EntitlementMissing(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamFailure {
    pub kind: String,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Effects {
    pub feedback: Option<Feedback>,
    pub usage: Option<Usage>,
    pub failure: Option<StreamFailure>,
}
