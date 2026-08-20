#![cfg(target_arch = "wasm32")]
#![allow(clippy::too_many_arguments)]

use std::cell::RefCell;

use gpt_codex_plugin_common::{
    Effects as CommonEffects, Feedback as CommonFeedback, Header as CommonHeader,
    LimitFeedback as CommonLimitFeedback, StreamFailure as CommonStreamFailure,
    Usage as CommonUsage,
    headers::{ResponseHeaderMode, sanitize_response_headers},
    response::{effects_from_raw_json, is_terminal_event, transform_response_value},
    sse::JsonSseItem,
};

wit_bindgen::generate!({
    path: "../../../srv/wit/stream-response-transformer.wit",
    world: "gpt-stream-response-transformer",
});

use aestus::stream_response_transformer::common_types::{
    Header, LimitFeedback, ResponseEffects, StreamFailure, TokenUsage, UpstreamFeedback,
};

#[derive(Default)]
struct StreamState {
    started: bool,
    feedback_emitted: bool,
}

thread_local! {
    /// 宿主为每条上游响应实例化独立 Component；thread-local 仅承载该实例内部的
    /// start/item/finish 生命周期状态，不在请求之间共享任何业务数据。
    static STATE: RefCell<StreamState> = RefCell::new(StreamState::default());
}

struct GptCodexStreamResponsePlugin;

impl Guest for GptCodexStreamResponsePlugin {
    fn start(input: StartInput) -> Result<ResponseHead, TransformError> {
        let StartInput { head, .. } = input;
        STATE.with(|state| {
            *state.borrow_mut() = StreamState {
                started: true,
                feedback_emitted: false,
            };
        });
        Ok(ResponseHead {
            status: head.status,
            headers: sanitize_response_headers(
                head.headers.into_iter().map(to_common_header).collect(),
                ResponseHeaderMode::EventStream,
            )
            .into_iter()
            .map(from_common_header)
            .collect(),
        })
    }

    fn transform_item(item: Vec<u8>) -> Result<ItemOutput, TransformError> {
        let started = STATE.with(|state| state.borrow().started);
        if !started {
            return Err(plugin_error(
                "stream_not_started",
                "宿主必须先调用 start，再迭代 SSE item",
            ));
        }

        let Some(mut parsed) = JsonSseItem::parse(&item).map_err(|message| {
            plugin_error(
                "invalid_sse_item",
                format!("无法解析完整 SSE item: {message}"),
            )
        })?
        else {
            return Ok(ItemOutput {
                item: Some(item),
                effects: empty_effects(),
            });
        };

        // usage、maintenance 和 failure 必须先读取原始上游 JSON；随后 response.failed
        // 可以安全删除下游不需要的大字段，且宿主不需要二次扫描改造后的 item。
        let mut effects = effects_from_raw_json(parsed.value(), Some(200), true);
        if !is_terminal_event(parsed.value()) {
            // 标准 Responses 只有终止事件的 usage 是累计快照。忽略中间扩展字段，避免
            // 某些兼容上游发送 delta usage 时违反宿主的单调累计约束。
            effects.usage = None;
        }
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            if effects.feedback.is_some() {
                if state.feedback_emitted {
                    effects.feedback = None;
                } else {
                    state.feedback_emitted = true;
                }
            }
        });

        let changed = transform_response_value(parsed.value_mut());
        let item = if changed {
            parsed
                .render()
                .map_err(|message| plugin_error("serialize_sse_item_failed", message))?
        } else {
            item
        };
        Ok(ItemOutput {
            item: Some(item),
            effects: from_common_effects(effects),
        })
    }

    fn finish() -> Result<FinishOutput, TransformError> {
        let started = STATE.with(|state| {
            let started = state.borrow().started;
            *state.borrow_mut() = StreamState::default();
            started
        });
        if !started {
            return Err(plugin_error(
                "stream_not_started",
                "宿主必须先调用 start，再调用 finish",
            ));
        }
        Ok(FinishOutput {
            items: Vec::new(),
            effects: empty_effects(),
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

fn empty_effects() -> ResponseEffects {
    ResponseEffects {
        feedback: None,
        usage: None,
        failure: None,
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

fn plugin_error(code: impl Into<String>, message: impl Into<String>) -> TransformError {
    TransformError {
        code: code.into(),
        message: message.into(),
    }
}

export!(GptCodexStreamResponsePlugin);
