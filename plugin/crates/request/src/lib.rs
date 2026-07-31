#![cfg(target_arch = "wasm32")]
// WIT canonical ABI 会为扁平 record 生成多参数内部函数，签名由 wit-bindgen 决定。
#![allow(clippy::too_many_arguments)]

use gpt_codex_plugin_common::{
    Header as CommonHeader, headers::build_account_headers, request::transform_oauth_body,
};

wit_bindgen::generate!({
    path: "../../../srv/wit/request-transformer.wit",
    world: "gpt-request-transformer",
});

use aestus::request_transformer::common_types::{Header, ResponseMode};

struct GptCodexRequestPlugin;

impl Guest for GptCodexRequestPlugin {
    fn transform(input: TransformInput) -> Result<TransformOutput, TransformError> {
        let TransformInput {
            account,
            headers,
            body,
        } = input;
        let headers = headers.into_iter().map(to_common_header).collect();

        let transformed = transform_oauth_body(&body)
            .map_err(|message| plugin_error("invalid_oauth_responses_body", message))?;
        let headers = build_account_headers(
            headers,
            &account.access_token,
            account.chatgpt_account_id.as_deref(),
            account.chatgpt_account_is_fedramp,
        );
        // ChatGPT Codex internal HTTP Responses 固定使用 SSE，但 response-mode
        // 描述的是下游交付方式：原始 stream=false 时宿主必须完整收集上游 SSE，
        // 再交给 buffered 插件转换成一个 Responses JSON。
        let response_mode = if transformed.downstream_streaming {
            ResponseMode::Stream
        } else {
            ResponseMode::Buffered
        };

        Ok(TransformOutput {
            headers: headers.into_iter().map(from_common_header).collect(),
            body: transformed.body,
            response_mode,
            response_context: transformed.response_context,
        })
    }
}

fn to_common_header(header: Header) -> CommonHeader {
    CommonHeader {
        name: header.name,
        value: header.value,
    }
}

fn from_common_header(header: CommonHeader) -> Header {
    Header {
        name: header.name,
        value: header.value,
    }
}

fn plugin_error(code: impl Into<String>, message: impl Into<String>) -> TransformError {
    TransformError {
        code: code.into(),
        message: message.into(),
    }
}

export!(GptCodexRequestPlugin);
