//! Codex OAuth buffered 响应转换插件的独立业务实现。

mod response;
mod responses_sse;

#[cfg(target_arch = "wasm32")]
mod component;

use std::collections::BTreeSet;

use response::{effects_from_raw_json, transform_response_value};
use responses_sse::convert_responses_sse_to_json;
use serde_json::Value;

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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Effects {
    pub feedback: Option<Feedback>,
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<Header>,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BufferedTransformInput {
    pub response: HttpResponse,
    pub request_context: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BufferedDisposition {
    Respond(HttpResponse),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BufferedTransformOutput {
    pub disposition: BufferedDisposition,
    pub effects: Effects,
}

/// 执行完整的 buffered 响应插件逻辑，不调用请求或流式响应插件中的任何业务实现。
pub fn transform_buffered_response(
    input: BufferedTransformInput,
) -> Result<BufferedTransformOutput, PluginError> {
    let BufferedTransformInput {
        response,
        request_context: _,
    } = input;
    let HttpResponse {
        status: upstream_status,
        headers,
        body,
    } = response;
    let mut converted_from_sse = false;
    let parsed_json = serde_json::from_slice::<Value>(&body).ok();
    let (mut parsed, effects) = if let Some(value) = parsed_json {
        let effects = effects_from_raw_json(&value, Some(upstream_status));
        (Some(value), effects)
    } else if (200..300).contains(&upstream_status) {
        let converted = convert_responses_sse_to_json(&body, upstream_status)
            .map_err(|message| {
                PluginError::new(
                    "invalid_upstream_responses_sse",
                    format!("上游 buffered Responses SSE 无法转换: {message}"),
                )
            })?
            .ok_or_else(|| {
                PluginError::new(
                    "invalid_upstream_responses_body",
                    "上游成功响应既不是 JSON，也不包含 SSE framing",
                )
            })?;
        converted_from_sse = true;
        (Some(converted.value), converted.effects)
    } else {
        (
            None,
            effects_from_raw_json(&Value::Null, Some(upstream_status)),
        )
    };

    let body = if let Some(value) = parsed.as_mut() {
        let transformed = !converted_from_sse && transform_response_value(value);
        if converted_from_sse || transformed {
            serde_json::to_vec(value).map_err(|error| {
                PluginError::new(
                    "serialize_response_failed",
                    format!("改造后的非流式响应无法序列化: {error}"),
                )
            })?
        } else {
            body
        }
    } else {
        body
    };
    let header_mode = if (200..300).contains(&upstream_status) {
        ResponseHeaderMode::Json
    } else {
        ResponseHeaderMode::Preserve
    };

    Ok(BufferedTransformOutput {
        disposition: BufferedDisposition::Respond(HttpResponse {
            status: upstream_status,
            headers: sanitize_response_headers(headers, header_mode),
            body,
        }),
        effects,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResponseHeaderMode {
    Preserve,
    Json,
}

fn sanitize_response_headers(headers: Vec<Header>, mode: ResponseHeaderMode) -> Vec<Header> {
    let connection_scoped = connection_scoped_header_names(&headers);
    let mut output = headers
        .into_iter()
        .filter(|header| {
            let name = header.name.to_ascii_lowercase();
            !is_hop_by_hop(&name)
                && !connection_scoped.contains(&name)
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
    if mode == ResponseHeaderMode::Json {
        append_unique_text(
            &mut output,
            "content-type",
            "application/json; charset=utf-8",
        );
    }
    output
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
