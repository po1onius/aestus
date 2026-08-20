#![cfg(target_arch = "wasm32")]
#![allow(clippy::too_many_arguments)]

use gpt_codex_plugin_common::{
    Effects as CommonEffects, Feedback as CommonFeedback, Header as CommonHeader,
    LimitFeedback as CommonLimitFeedback, Usage as CommonUsage,
    headers::{ResponseHeaderMode, sanitize_response_headers},
    response::{effects_from_raw_json, transform_response_value},
    responses_sse::convert_responses_sse_to_json,
};

wit_bindgen::generate!({
    path: "../../../srv/wit/buffered-response-transformer.wit",
    world: "gpt-buffered-response-transformer",
});

use aestus::buffered_response_transformer::common_types::{
    BufferedDisposition, Header, LimitFeedback, Response, ResponseEffects, TokenUsage,
    UpstreamFeedback,
};

struct GptCodexBufferedResponsePlugin;

impl Guest for GptCodexBufferedResponsePlugin {
    fn transform(input: TransformInput) -> Result<TransformOutput, TransformError> {
        let TransformInput {
            response: input, ..
        } = input;
        let Response {
            status: upstream_status,
            headers,
            body,
        } = input;
        let mut status = upstream_status;
        let mut converted_from_sse = false;
        let parsed_json = serde_json::from_slice::<serde_json::Value>(&body).ok();
        let (mut parsed, effects) = if let Some(value) = parsed_json {
            let effects = effects_from_raw_json(&value, Some(upstream_status), false);
            (Some(value), effects)
        } else if (200..300).contains(&upstream_status)
            && let Some(converted) = convert_responses_sse_to_json(&body, upstream_status)
        {
            status = converted.status;
            converted_from_sse = true;
            (Some(converted.value), converted.effects)
        } else {
            (
                None,
                effects_from_raw_json(&serde_json::Value::Null, Some(upstream_status), false),
            )
        };

        let body = if let Some(value) = parsed.as_mut() {
            let transformed = transform_response_value(value);
            if converted_from_sse || transformed {
                serde_json::to_vec(value).map_err(|error| TransformError {
                    code: "serialize_response_failed".to_owned(),
                    message: format!("改造后的非流式响应无法序列化: {error}"),
                })?
            } else {
                body
            }
        } else {
            // 非 JSON 错误页或空 body 不臆测业务协议，但 header 仍由插件安全过滤，HTTP
            // status 仍可通过 effects 生成 maintenance 回执。
            body
        };
        let header_mode = if converted_from_sse {
            ResponseHeaderMode::Json
        } else {
            ResponseHeaderMode::Preserve
        };
        let headers = sanitize_response_headers(
            headers.into_iter().map(to_common_header).collect(),
            header_mode,
        )
        .into_iter()
        .map(from_common_header)
        .collect();

        Ok(TransformOutput {
            disposition: BufferedDisposition::Respond(Response {
                status,
                headers,
                body,
            }),
            effects: from_common_effects(effects),
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
