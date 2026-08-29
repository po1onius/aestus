#![allow(clippy::too_many_arguments)]

use std::cell::RefCell;

use crate::{
    Effects as CommonEffects, Feedback as CommonFeedback, Header as CommonHeader,
    LimitFeedback as CommonLimitFeedback, ResponseHead as CommonResponseHead,
    StreamFailure as CommonStreamFailure, StreamResponseTransformer,
    StreamStartInput as CommonStartInput, Usage as CommonUsage,
};

wit_bindgen::generate!({
    path: "../../../srv/wit/stream-response-transformer.wit",
    world: "gpt-stream-response-transformer",
});

use aestus::stream_response_transformer::common_types::{
    Header, LimitFeedback, ResponseEffects, StreamFailure, TokenUsage, UpstreamFeedback,
};

thread_local! {
    /// 宿主为每条上游响应实例化独立 Component；thread-local 仅承载该实例内部的
    /// start/item/finish 生命周期状态，不在请求之间共享任何业务数据。
    static TRANSFORMER: RefCell<StreamResponseTransformer> =
        RefCell::new(StreamResponseTransformer::default());
}

struct GptCodexStreamResponsePlugin;

impl Guest for GptCodexStreamResponsePlugin {
    fn start(input: StartInput) -> Result<ResponseHead, TransformError> {
        let transformed = TRANSFORMER.with(|transformer| {
            transformer.borrow_mut().start(CommonStartInput {
                head: CommonResponseHead {
                    status: input.head.status,
                    headers: input
                        .head
                        .headers
                        .into_iter()
                        .map(to_common_header)
                        .collect(),
                },
                request_context: input.request_context,
            })
        });
        let transformed = transformed.map_err(from_common_error)?;
        Ok(ResponseHead {
            status: transformed.status,
            headers: transformed
                .headers
                .into_iter()
                .map(from_common_header)
                .collect(),
        })
    }

    fn transform_item(item: Vec<u8>) -> Result<ItemOutput, TransformError> {
        let transformed = TRANSFORMER
            .with(|transformer| transformer.borrow_mut().transform_item(item))
            .map_err(from_common_error)?;
        Ok(ItemOutput {
            item: transformed.item,
            effects: from_common_effects(transformed.effects),
        })
    }

    fn finish() -> Result<FinishOutput, TransformError> {
        let transformed = TRANSFORMER
            .with(|transformer| transformer.borrow_mut().finish())
            .map_err(from_common_error)?;
        Ok(FinishOutput {
            items: transformed.items,
            effects: from_common_effects(transformed.effects),
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

fn from_common_effects(effects: CommonEffects) -> ResponseEffects {
    ResponseEffects {
        feedback: effects.feedback.map(from_common_feedback),
        usage: effects.usage.map(from_common_usage),
        failure: effects.failure.map(from_common_failure),
    }
}

fn from_common_feedback(feedback: CommonFeedback) -> UpstreamFeedback {
    match feedback {
        CommonFeedback::Error(reason) => UpstreamFeedback::Error(reason),
        CommonFeedback::AuthenticationRejected(reason) => {
            UpstreamFeedback::AuthenticationRejected(reason)
        }
        CommonFeedback::RateLimited(limit) => {
            UpstreamFeedback::RateLimited(from_common_limit(limit))
        }
        CommonFeedback::QuotaExhausted(limit) => {
            UpstreamFeedback::QuotaExhausted(from_common_limit(limit))
        }
        CommonFeedback::TemporarilyUnavailable(reason) => {
            UpstreamFeedback::TemporarilyUnavailable(reason)
        }
        CommonFeedback::EntitlementMissing(reason) => UpstreamFeedback::EntitlementMissing(reason),
    }
}

fn from_common_limit(limit: CommonLimitFeedback) -> LimitFeedback {
    LimitFeedback {
        resets_at_unix_seconds: limit.resets_at_unix_seconds,
        reason: limit.reason,
    }
}

fn from_common_usage(usage: CommonUsage) -> TokenUsage {
    TokenUsage {
        input_tokens: usage.input_tokens,
        cached_input_tokens: usage.cached_input_tokens,
        output_tokens: usage.output_tokens,
        reasoning_output_tokens: usage.reasoning_output_tokens,
        total_tokens: usage.total_tokens,
    }
}

fn from_common_failure(failure: CommonStreamFailure) -> StreamFailure {
    StreamFailure {
        kind: failure.kind,
        message: failure.message,
    }
}

fn from_common_error(error: crate::PluginError) -> TransformError {
    TransformError {
        code: error.code,
        message: error.message,
    }
}

export!(GptCodexStreamResponsePlugin);
