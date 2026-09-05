//! Codex OAuth streaming 响应转换插件的独立业务实现。

mod response;

#[cfg(target_arch = "wasm32")]
mod component;

use std::collections::BTreeSet;

use gpt_codex_plugin_utils::sse::JsonSseItem;
use response::{
    effects_from_raw_json, is_terminal_event, requires_client_retry_event, transform_response_value,
};

const CLIENT_RETRY_FAILED_EVENT: &[u8] = br#"event: response.failed
data: {"type":"response.failed","response":{"error":{"code":"rate_limit_exceeded","message":"Please try again in 28ms"}}}

"#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginError {
    pub code: String,
    pub message: String,
}

impl PluginError {
    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for PluginError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for PluginError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub name: String,
    pub value: Vec<u8>,
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponseHead {
    pub status: u16,
    pub headers: Vec<Header>,
}

/// 当前套件约定的上下文字段；额外 JSON 字段由插件自行扩展。
fn parse_context_stream(context: Option<&[u8]>) -> Result<Option<bool>, PluginError> {
    let Some(context) = context else {
        return Ok(None);
    };
    let value: serde_json::Value = serde_json::from_slice(context).map_err(|error| {
        PluginError::new(
            "invalid_plugin_context",
            format!(
                "plugin-context 不是合法 JSON（行 {}，列 {}）",
                error.line(),
                error.column()
            ),
        )
    })?;
    value
        .get("stream")
        .and_then(serde_json::Value::as_bool)
        .map(Some)
        .ok_or_else(|| {
            PluginError::new(
                "invalid_plugin_context",
                "plugin-context.stream 必须是布尔值",
            )
        })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamStartInput {
    pub head: ResponseHead,
    pub plugin_context: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamItemOutput {
    pub item: Option<Vec<u8>>,
    pub effects: Effects,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamFinishOutput {
    pub items: Vec<Vec<u8>>,
    pub effects: Effects,
}

/// 一条上游 SSE 响应对应一个实例，生命周期状态完全归流式插件所有。
#[derive(Debug, Default)]
pub struct StreamResponseTransformer {
    started: bool,
    feedback_emitted: bool,
}

impl StreamResponseTransformer {
    pub fn start(&mut self, input: StreamStartInput) -> Result<ResponseHead, PluginError> {
        // 读取本套件的私有字段；不据此限制宿主选槽或补齐上游 Response。
        let _stream = parse_context_stream(input.plugin_context.as_deref())?;
        self.started = true;
        self.feedback_emitted = false;
        Ok(ResponseHead {
            status: input.head.status,
            headers: sanitize_response_headers(input.head.headers),
        })
    }

    pub fn transform_item(&mut self, item: Vec<u8>) -> Result<StreamItemOutput, PluginError> {
        if !self.started {
            return Err(PluginError::new(
                "stream_not_started",
                "调用方必须先调用 start，再迭代 SSE item",
            ));
        }

        let Some(mut parsed) = JsonSseItem::parse(&item).map_err(|message| {
            PluginError::new(
                "invalid_sse_item",
                format!("无法解析完整 SSE item: {message}"),
            )
        })?
        else {
            return Ok(StreamItemOutput {
                item: Some(item),
                effects: Effects::default(),
            });
        };

        let mut effects = effects_from_raw_json(parsed.value(), Some(200), true);
        if !is_terminal_event(parsed.value()) {
            effects.usage = None;
        }
        if effects.feedback.is_some() {
            if self.feedback_emitted {
                effects.feedback = None;
            } else {
                self.feedback_emitted = true;
            }
        }

        let replace_with_client_retry = requires_client_retry_event(parsed.value());
        let changed = !replace_with_client_retry && transform_response_value(parsed.value_mut());
        let item = if replace_with_client_retry {
            CLIENT_RETRY_FAILED_EVENT.to_vec()
        } else if changed {
            parsed
                .render()
                .map_err(|message| PluginError::new("serialize_sse_item_failed", message))?
        } else {
            item
        };
        Ok(StreamItemOutput {
            item: Some(item),
            effects,
        })
    }

    pub fn finish(&mut self) -> Result<StreamFinishOutput, PluginError> {
        if !self.started {
            return Err(PluginError::new(
                "stream_not_started",
                "调用方必须先调用 start，再调用 finish",
            ));
        }
        *self = Self::default();
        Ok(StreamFinishOutput {
            items: Vec::new(),
            effects: Effects::default(),
        })
    }
}

fn sanitize_response_headers(headers: Vec<Header>) -> Vec<Header> {
    let connection_scoped = connection_scoped_header_names(&headers);
    let mut output = headers
        .into_iter()
        .filter(|header| {
            let name = header.name.to_ascii_lowercase();
            !is_hop_by_hop(&name)
                && !connection_scoped.contains(&name)
                && !is_codex_private_header(&name)
                && !matches!(
                    name.as_str(),
                    "host"
                        | "content-length"
                        | "set-cookie"
                        | "authorization"
                        | "x-api-key"
                        | "proxy-authenticate"
                        | "proxy-authorization"
                )
        })
        .collect::<Vec<_>>();
    append_unique_text(
        &mut output,
        "content-type",
        "text/event-stream; charset=utf-8",
    );
    append_unique_text(&mut output, "cache-control", "no-cache");
    append_unique_text(&mut output, "connection", "keep-alive");
    output
}

/// ChatGPT Codex 上游的私有响应 header 不属于公开 Responses API，也可能包含账号额度、
/// 会话状态或内部路由信息。匹配时同时拦截没有后缀的保留名称，避免未来新增 header 时
/// 需要逐项维护黑名单。
fn is_codex_private_header(name: &str) -> bool {
    name == "x-codex" || name.starts_with("x-codex-")
}

fn connection_scoped_header_names(headers: &[Header]) -> BTreeSet<String> {
    headers
        .iter()
        .filter(|header| header.name.eq_ignore_ascii_case("connection"))
        .filter_map(|header| std::str::from_utf8(&header.value).ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

fn is_hop_by_hop(name: &str) -> bool {
    matches!(
        name,
        "connection"
            | "transfer-encoding"
            | "te"
            | "trailer"
            | "upgrade"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "proxy-connection"
    )
}

fn append_unique_text(headers: &mut Vec<Header>, name: &str, value: &str) {
    headers.retain(|header| !header.name.eq_ignore_ascii_case(name));
    headers.push(Header {
        name: name.to_owned(),
        value: value.as_bytes().to_vec(),
    });
}
