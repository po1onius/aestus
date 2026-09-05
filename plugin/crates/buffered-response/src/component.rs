#![allow(clippy::too_many_arguments)]

use crate::{
    BufferedDisposition as CommonDisposition, BufferedTransformInput as CommonTransformInput,
    Effects as CommonEffects, Feedback as CommonFeedback, Header as CommonHeader,
    HttpResponse as CommonResponse, LimitFeedback as CommonLimitFeedback, Usage as CommonUsage,
    transform_buffered_response,
};

wit_bindgen::generate!({
    path: [
        "../../../srv/wit/plugin-types.wit",
        "../../../srv/wit/buffered-response-transformer.wit",
    ],
    world: "aestus:buffered-response-transformer/gpt-buffered-response-transformer@1.0.0",
    with: {
        "aestus:plugin-types/response-types@1.0.0": generate,
    },
});

use aestus::buffered_response_transformer::common_types::{
    BufferedDisposition, Header, LimitFeedback, Response, ResponseEffects, TokenUsage,
    UpstreamFeedback,
};

struct GptCodexBufferedResponsePlugin;

impl Guest for GptCodexBufferedResponsePlugin {
    fn transform(input: TransformInput) -> Result<TransformOutput, TransformError> {
        let transformed = transform_buffered_response(to_common_input(input)).map_err(|error| {
            TransformError {
                code: error.code,
                message: error.message,
            }
        })?;
        let disposition = match transformed.disposition {
            CommonDisposition::Respond(response) => {
                BufferedDisposition::Respond(from_common_response(response))
            }
        };

        Ok(TransformOutput {
            disposition,
            effects: from_common_effects(transformed.effects),
        })
    }
}

fn to_common_input(input: TransformInput) -> CommonTransformInput {
    CommonTransformInput {
        response: CommonResponse {
            status: input.response.status,
            headers: input
                .response
                .headers
                .into_iter()
                .map(to_common_header)
                .collect(),
            body: input.response.body,
        },
        plugin_context: input.plugin_context,
    }
}

fn from_common_response(response: CommonResponse) -> Response {
    Response {
        status: response.status,
        headers: response
            .headers
            .into_iter()
            .map(from_common_header)
            .collect(),
        body: response.body,
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

export!(GptCodexBufferedResponsePlugin);
