//! 可由普通 Rust 程序和 WASM ABI 入口共同调用的插件函数。
//!
//! 这些类型刻意不依赖 wit-bindgen。WASM 组件只负责把 WIT record/variant 映射到这里，
//! 测试服务也直接调用同一组函数，从而保证本地验证覆盖的就是实际插件能力。

use serde_json::Value;

use crate::{
    Effects, Header,
    headers::{
        ResponseHeaderMode, build_account_headers, build_image_account_headers,
        sanitize_response_headers,
    },
    images::{transform_edits_body, transform_generations_body, transform_image_response_body},
    request::transform_oauth_body,
    response::{effects_from_raw_json, is_terminal_event, transform_response_value},
    responses_sse::{convert_responses_sse_to_json, validate_non_streaming_response},
    sse::JsonSseItem,
};

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
pub struct AccountResource {
    pub access_token: String,
    pub chatgpt_account_id: Option<String>,
    pub chatgpt_account_is_fedramp: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseMode {
    Stream,
    Buffered,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestTransformInput {
    pub account: AccountResource,
    pub headers: Vec<Header>,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestTransformOutput {
    pub headers: Vec<Header>,
    pub body: Vec<u8>,
    pub response_mode: ResponseMode,
    pub response_context: Option<Vec<u8>>,
}

/// Images 请求函数沿用 Responses 插件的账号输入，但输出固定是发往 Codex Images
/// 端点的 JSON header/body，不需要 response mode 或会话上下文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageRequestTransformInput {
    pub account: AccountResource,
    pub headers: Vec<Header>,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageRequestTransformOutput {
    pub headers: Vec<Header>,
    pub body: Vec<u8>,
}

/// 转换标准 OpenAI `/images/generations` JSON 请求。
pub fn transform_image_generations_request(
    input: ImageRequestTransformInput,
) -> Result<ImageRequestTransformOutput, PluginError> {
    let body = transform_generations_body(&input.body)
        .map_err(|message| PluginError::new("invalid_image_generations_request", message))?;
    Ok(ImageRequestTransformOutput {
        headers: build_image_account_headers(
            input.headers,
            &input.account.access_token,
            input.account.chatgpt_account_id.as_deref(),
            input.account.chatgpt_account_is_fedramp,
        ),
        body,
    })
}

/// 转换标准 OpenAI `/images/edits` multipart 请求。解析过程是异步的，便于底层
/// multipart 库按字段消费 body；函数输出仍是可直接发送给 Codex 的完整 JSON。
pub async fn transform_image_edits_request(
    input: ImageRequestTransformInput,
) -> Result<ImageRequestTransformOutput, PluginError> {
    let content_type = input
        .headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case("content-type"))
        .and_then(|header| std::str::from_utf8(&header.value).ok())
        .ok_or_else(|| {
            PluginError::new(
                "invalid_image_edits_request",
                "图片编辑请求缺少 Content-Type",
            )
        })?
        .to_owned();
    let body = transform_edits_body(&content_type, input.body)
        .await
        .map_err(|message| PluginError::new("invalid_image_edits_request", message))?;
    Ok(ImageRequestTransformOutput {
        headers: build_image_account_headers(
            input.headers,
            &input.account.access_token,
            input.account.chatgpt_account_id.as_deref(),
            input.account.chatgpt_account_is_fedramp,
        ),
        body,
    })
}

/// 将成功的 Codex 图片响应转成最小 OpenAI Images 响应。非成功响应不改写错误 body，
/// 只执行通用响应 header 安全过滤，便于下游看到上游的真实错误信息。
pub fn transform_image_response(response: HttpResponse) -> Result<HttpResponse, PluginError> {
    let HttpResponse {
        status,
        headers,
        body,
    } = response;
    let successful = (200..300).contains(&status);
    let body = if successful {
        transform_image_response_body(&body)
            .map_err(|message| PluginError::new("invalid_codex_image_response", message))?
    } else {
        body
    };
    Ok(HttpResponse {
        status,
        headers: sanitize_response_headers(
            headers,
            if successful {
                ResponseHeaderMode::Json
            } else {
                ResponseHeaderMode::Preserve
            },
        ),
        body,
    })
}

/// 执行完整的 Codex OAuth 请求插件逻辑。调用方拿到的 header/body 可以直接发送到
/// `backend-api/codex/responses`，`response_mode` 描述响应应交给哪个响应函数处理。
pub fn transform_request(
    input: RequestTransformInput,
) -> Result<RequestTransformOutput, PluginError> {
    let transformed = transform_oauth_body(&input.body)
        .map_err(|message| PluginError::new("invalid_oauth_responses_body", message))?;
    let headers = build_account_headers(
        input.headers,
        &input.account.access_token,
        input.account.chatgpt_account_id.as_deref(),
        input.account.chatgpt_account_is_fedramp,
    );
    let response_mode = if transformed.downstream_streaming {
        ResponseMode::Stream
    } else {
        ResponseMode::Buffered
    };

    Ok(RequestTransformOutput {
        headers,
        body: transformed.body,
        response_mode,
        response_context: None,
    })
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

/// 执行完整的非流式响应插件逻辑，包括上游 SSE 到 Responses JSON 的转换、响应字段
/// 修正、敏感 header 清理和 usage/maintenance effects 提取。
pub fn transform_buffered_response(
    input: BufferedTransformInput,
) -> Result<BufferedTransformOutput, PluginError> {
    let HttpResponse {
        status: upstream_status,
        headers,
        body,
    } = input.response;
    let mut status = upstream_status;
    let mut converted_from_sse = false;
    let parsed_json = serde_json::from_slice::<Value>(&body).ok();
    let (mut parsed, effects) = if let Some(value) = parsed_json {
        // HTTP 2xx 的 buffered Responses 必须直接是标准 Response object。若上游返回了
        // SSE event envelope、HTML 或业务错误 JSON，不能继续以成功响应透传给客户端。
        if (200..300).contains(&upstream_status) {
            validate_non_streaming_response(&value).map_err(|message| {
                PluginError::new(
                    "invalid_upstream_responses_json",
                    format!("上游非流式 Responses JSON 结构非法: {message}"),
                )
            })?;
        }
        let effects = effects_from_raw_json(&value, Some(upstream_status), false);
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
                    "上游成功响应既不是标准 Responses JSON，也不包含 SSE framing",
                )
            })?;
        status = converted.status;
        converted_from_sse = true;
        (Some(converted.value), converted.effects)
    } else {
        (
            None,
            effects_from_raw_json(&Value::Null, Some(upstream_status), false),
        )
    };

    let body = if let Some(value) = parsed.as_mut() {
        let transformed = transform_response_value(value);
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
        // 非 JSON 错误页或空 body 不臆测业务协议，但 header 仍按插件契约执行安全过滤。
        body
    };
    // 能走到这里的 2xx body 已经确认或转换为 Response JSON，统一输出 JSON Content-Type，
    // 避免上游错误标注为 text/event-stream 后让非流式客户端按 SSE 解析。
    let header_mode = if (200..300).contains(&status) {
        ResponseHeaderMode::Json
    } else {
        ResponseHeaderMode::Preserve
    };

    Ok(BufferedTransformOutput {
        disposition: BufferedDisposition::Respond(HttpResponse {
            status,
            headers: sanitize_response_headers(headers, header_mode),
            body,
        }),
        effects,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponseHead {
    pub status: u16,
    pub headers: Vec<Header>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamStartInput {
    pub head: ResponseHead,
    pub request_context: Option<Vec<u8>>,
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

/// 一条上游 SSE 响应对应一个实例。状态由调用方显式持有，普通 Rust 服务不需要模拟
/// WASM Component 的实例隔离；WASM 入口则把实例放在线程局部存储中维持原有生命周期。
#[derive(Debug, Default)]
pub struct StreamResponseTransformer {
    started: bool,
    feedback_emitted: bool,
}

impl StreamResponseTransformer {
    pub fn start(&mut self, input: StreamStartInput) -> Result<ResponseHead, PluginError> {
        self.started = true;
        self.feedback_emitted = false;
        Ok(ResponseHead {
            status: input.head.status,
            headers: sanitize_response_headers(input.head.headers, ResponseHeaderMode::EventStream),
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

        // effects 必须先从上游原始事件提取，防止后续响应瘦身删除 usage/error 字段。
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

        let changed = transform_response_value(parsed.value_mut());
        let item = if changed {
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
