use std::borrow::Cow;

use base64::{Engine as _, engine::general_purpose::STANDARD};

/// 可安全写入 tracing、同时保留全部原始字节的 provider 响应正文。
///
/// provider 的正常错误响应通常是 UTF-8 JSON 或 SSE，此时直接记录原文便于检索和阅读。
/// 如果上游返回了非 UTF-8 数据，则改用标准 Base64 编码并单独记录编码类型，避免
/// `String::from_utf8_lossy` 用替换字符破坏原始响应，保证排障时可以完整还原正文。
pub(crate) struct TracingResponseBody<'a> {
    content: Cow<'a, str>,
    encoding: &'static str,
}

impl TracingResponseBody<'_> {
    pub(crate) fn content(&self) -> &str {
        self.content.as_ref()
    }

    pub(crate) fn encoding(&self) -> &'static str {
        self.encoding
    }
}

/// 将 provider 错误响应正文转换为 tracing 字段。
///
/// 本函数不截断内容。调用方只应在已经确认响应属于错误响应或错误 SSE 事件后调用，避免
/// 把正常模型输出写入运行日志。
pub(crate) fn response_body_for_tracing(body: &[u8]) -> TracingResponseBody<'_> {
    match std::str::from_utf8(body) {
        Ok(content) => TracingResponseBody {
            content: Cow::Borrowed(content),
            encoding: "utf-8",
        },
        Err(_) => TracingResponseBody {
            content: Cow::Owned(STANDARD.encode(body)),
            encoding: "base64",
        },
    }
}
