#![cfg(target_arch = "wasm32")]
// WIT canonical ABI 会为扁平 record 生成多参数内部函数，签名由 wit-bindgen 决定。
#![allow(clippy::too_many_arguments)]

use gpt_codex_plugin_common::{
    Header as CommonHeader,
    functions::{
        AccountResource as CommonAccountResource, RequestTransformInput as CommonTransformInput,
        ResponseMode as CommonResponseMode, transform_request,
    },
};

wit_bindgen::generate!({
    path: "../../../srv/wit/request-transformer.wit",
    world: "gpt-request-transformer",
});

use aestus::request_transformer::common_types::{Header, ResponseMode};

struct GptCodexRequestPlugin;

impl Guest for GptCodexRequestPlugin {
    fn transform(input: TransformInput) -> Result<TransformOutput, TransformError> {
        let transformed = transform_request(to_common_input(input))
            .map_err(|error| plugin_error(error.code, error.message))?;
        let response_mode = match transformed.response_mode {
            CommonResponseMode::Stream => ResponseMode::Stream,
            CommonResponseMode::Buffered => ResponseMode::Buffered,
        };

        Ok(TransformOutput {
            headers: transformed
                .headers
                .into_iter()
                .map(from_common_header)
                .collect(),
            body: transformed.body,
            response_mode,
            response_context: transformed.response_context,
        })
    }
}

fn to_common_input(input: TransformInput) -> CommonTransformInput {
    CommonTransformInput {
        account: CommonAccountResource {
            access_token: input.account.access_token,
            chatgpt_account_id: input.account.chatgpt_account_id,
            chatgpt_account_is_fedramp: input.account.chatgpt_account_is_fedramp,
        },
        headers: input.headers.into_iter().map(to_common_header).collect(),
        body: input.body,
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
