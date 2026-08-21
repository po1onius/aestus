use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};

use crate::{
    Effects,
    response::effects_from_raw_json,
    sse::{body_has_sse_framing, for_each_json_data_value},
};

/// 成功的 SSE→JSON 转换结果。`effects` 在任何字段改造前从原始终止事件中提取，确保
/// usage 与 maintenance 不受下游响应瘦身或 output 修复影响。
pub struct ConvertedResponsesBody {
    pub status: u16,
    pub value: Value,
    pub effects: Effects,
}

/// 将完整的 OpenAI Responses SSE body 转成单个非流式 Responses JSON。
///
/// 与 sub2api 默认 OAuth HTTP 路径保持相同主序：优先取
/// `response.completed/done/incomplete` 中的 `response`；仅当其 output 为空时，先使用
/// 权威的 `output_item.done` 原始 item，完全没有 done item 才用文本、reasoning 和函数
/// 参数 delta 重建。`response.failed` 转成 502 JSON 错误。没有可识别终止事件时返回
/// `None`，由调用方保留原始 SSE 兜底。
pub fn convert_responses_sse_to_json(
    body: &[u8],
    upstream_status: u16,
) -> Option<ConvertedResponsesBody> {
    if !body_has_sse_framing(body) {
        return None;
    }

    let mut terminal_response_event = None::<Value>;
    let mut failed_event = None::<Value>;
    let mut done_items = Vec::<Value>::new();
    let mut seen_done_items = BTreeSet::<String>::new();
    for_each_json_data_value(body, |event| {
        let event_type = event.get("type").and_then(Value::as_str).map(str::trim);
        match event_type {
            Some("response.completed" | "response.done" | "response.incomplete")
                if terminal_response_event.is_none() =>
            {
                terminal_response_event = Some(event);
            }
            Some("response.failed") if failed_event.is_none() => {
                failed_event = Some(event);
            }
            Some("response.output_item.done") => {
                let Some(item) = event.get("item").filter(|item| item.is_object()) else {
                    return;
                };
                let key = item
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|id| !id.is_empty())
                    .map(str::to_owned)
                    .unwrap_or_else(|| item.to_string());
                if seen_done_items.insert(key) {
                    done_items.push(item.clone());
                }
            }
            _ => {}
        }
    });

    if let Some(mut terminal) = terminal_response_event {
        let effects = effects_from_raw_json(&terminal, Some(upstream_status), false);
        let mut response = terminal
            .get_mut("response")
            .filter(|response| response.is_object())
            .map(Value::take)?;
        if response_output_is_empty(&response) {
            let reconstructed = if done_items.is_empty() {
                reconstruct_output_from_deltas(body)
            } else {
                Some(done_items)
            };
            if let Some(output) = reconstructed
                && let Some(response) = response.as_object_mut()
            {
                response.insert("output".to_owned(), Value::Array(output));
            }
        }
        return Some(ConvertedResponsesBody {
            status: upstream_status,
            value: response,
            effects,
        });
    }

    let terminal = failed_event?;
    let effects = effects_from_raw_json(&terminal, Some(upstream_status), false);
    let message = response_error_message(&terminal)
        .unwrap_or_else(|| "OpenAI upstream response failed".to_owned());
    Some(ConvertedResponsesBody {
        status: 502,
        value: json!({
            "error": {
                "type": "upstream_error",
                "message": message,
            }
        }),
        effects,
    })
}

fn response_output_is_empty(response: &Value) -> bool {
    response
        .get("output")
        .and_then(Value::as_array)
        .is_none_or(Vec::is_empty)
}

#[derive(Default)]
struct DeltaOutputAccumulator {
    text: String,
    reasoning: String,
    function_calls: Vec<FunctionCall>,
    output_index_to_function: BTreeMap<i64, usize>,
}

struct FunctionCall {
    call_id: String,
    name: String,
    arguments: String,
}

fn reconstruct_output_from_deltas(body: &[u8]) -> Option<Vec<Value>> {
    let mut accumulator = DeltaOutputAccumulator::default();
    for_each_json_data_value(body, |event| {
        let event_type = event.get("type").and_then(Value::as_str).map(str::trim);
        match event_type {
            Some("response.output_text.delta") => {
                append_string_field(&mut accumulator.text, &event, "delta");
            }
            Some("response.reasoning_summary_text.delta" | "response.reasoning_text.delta") => {
                append_string_field(&mut accumulator.reasoning, &event, "delta");
            }
            Some("response.output_item.added") => {
                let Some(item) = event.get("item") else {
                    return;
                };
                if !matches!(
                    item.get("type").and_then(Value::as_str),
                    Some("function_call" | "custom_tool_call")
                ) {
                    return;
                }
                let output_index = event
                    .get("output_index")
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                let index = accumulator.function_calls.len();
                accumulator
                    .output_index_to_function
                    .insert(output_index, index);
                accumulator.function_calls.push(FunctionCall {
                    call_id: string_field(item, "call_id"),
                    name: string_field(item, "name"),
                    arguments: String::new(),
                });
            }
            Some(
                "response.function_call_arguments.delta" | "response.custom_tool_call_input.delta",
            ) => {
                let output_index = event
                    .get("output_index")
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                if let Some(index) = accumulator
                    .output_index_to_function
                    .get(&output_index)
                    .copied()
                    && let Some(call) = accumulator.function_calls.get_mut(index)
                {
                    append_string_field(&mut call.arguments, &event, "delta");
                }
            }
            _ => {}
        }
    });
    accumulator.build_output()
}

impl DeltaOutputAccumulator {
    fn build_output(self) -> Option<Vec<Value>> {
        let mut output = Vec::new();
        if !self.reasoning.is_empty() {
            output.push(json!({
                "type": "reasoning",
                "summary": [{"type":"summary_text", "text":self.reasoning}],
            }));
        }
        if !self.text.is_empty() {
            output.push(json!({
                "type": "message",
                "role": "assistant",
                "content": [{"type":"output_text", "text":self.text}],
            }));
        }
        for call in self.function_calls {
            let mut item =
                Map::from_iter([("type".to_owned(), Value::String("function_call".to_owned()))]);
            insert_non_empty(&mut item, "call_id", call.call_id);
            insert_non_empty(&mut item, "name", call.name);
            insert_non_empty(&mut item, "arguments", call.arguments);
            output.push(Value::Object(item));
        }
        (!output.is_empty()).then_some(output)
    }
}

fn append_string_field(output: &mut String, value: &Value, field: &str) {
    if let Some(delta) = value.get(field).and_then(Value::as_str) {
        output.push_str(delta);
    }
}

fn string_field(value: &Value, field: &str) -> String {
    value
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn insert_non_empty(object: &mut Map<String, Value>, field: &str, value: String) {
    if !value.is_empty() {
        object.insert(field.to_owned(), Value::String(value));
    }
}

fn response_error_message(value: &Value) -> Option<String> {
    let message = value
        .pointer("/response/error/message")
        .or_else(|| value.pointer("/error/message"))
        .or_else(|| value.get("message"))?
        .as_str()?
        .trim();
    if message.is_empty() {
        return None;
    }
    const MAX_CHARS: usize = 1_024;
    Some(message.chars().take(MAX_CHARS).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Feedback, Usage};

    #[test]
    fn completed_event_becomes_non_streaming_response() {
        let body = br#"data: {"type":"response.completed","response":{"id":"resp_1","object":"response","status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"hello"}]}],"usage":{"input_tokens":3,"output_tokens":2}}}

data: [DONE]

"#;
        let converted = convert_responses_sse_to_json(body, 200).unwrap();
        assert_eq!(converted.status, 200);
        assert_eq!(converted.value["id"], "resp_1");
        assert_eq!(converted.value["output"][0]["content"][0]["text"], "hello");
        assert_eq!(
            converted.effects.usage,
            Some(Usage {
                input_tokens: 3,
                cached_input_tokens: 0,
                output_tokens: 2,
                reasoning_output_tokens: 0,
                total_tokens: 5,
            })
        );
    }

    #[test]
    fn empty_terminal_output_prefers_raw_done_items() {
        let body = br#"data: {"type":"response.output_text.delta","delta":"should not win"}

data: {"type":"response.output_item.done","item":{"id":"rs_1","type":"reasoning","encrypted_content":"opaque","summary":[]}}

data: {"type":"response.output_item.done","item":{"id":"fc_1","type":"function_call","call_id":"call_1","name":"lookup","arguments":"{\"q\":1}","future_field":true}}

data: {"type":"response.completed","response":{"id":"resp_2","output":[],"usage":{"input_tokens":1,"output_tokens":1}}}

"#;
        let converted = convert_responses_sse_to_json(body, 200).unwrap();
        assert_eq!(converted.value["output"].as_array().unwrap().len(), 2);
        assert_eq!(converted.value["output"][0]["encrypted_content"], "opaque");
        assert_eq!(converted.value["output"][1]["future_field"], true);
        assert_eq!(converted.value["output"][1]["arguments"], r#"{"q":1}"#);
    }

    #[test]
    fn deltas_rebuild_text_reasoning_and_function_arguments() {
        let body = br#"data: {"type":"response.reasoning_summary_text.delta","delta":"think"}

data: {"type":"response.output_text.delta","delta":"hel"}

data: {"type":"response.output_text.delta","delta":"lo"}

data: {"type":"response.output_item.added","output_index":2,"item":{"type":"function_call","call_id":"call_1","name":"lookup"}}

data: {"type":"response.function_call_arguments.delta","output_index":2,"delta":"{\"q\":"}

data: {"type":"response.function_call_arguments.delta","output_index":2,"delta":"1}"}

data: {"type":"response.completed","response":{"id":"resp_3","output":[],"usage":{"input_tokens":1,"output_tokens":2}}}

"#;
        let converted = convert_responses_sse_to_json(body, 200).unwrap();
        assert_eq!(converted.value["output"][0]["summary"][0]["text"], "think");
        assert_eq!(converted.value["output"][1]["content"][0]["text"], "hello");
        assert_eq!(converted.value["output"][2]["name"], "lookup");
        assert_eq!(converted.value["output"][2]["arguments"], r#"{"q":1}"#);
    }

    #[test]
    fn failed_event_becomes_502_json_and_keeps_feedback() {
        let body = br#"data: {"type":"response.failed","response":{"error":{"code":"server_error","message":"overloaded"}}}

data: [DONE]

"#;
        let converted = convert_responses_sse_to_json(body, 200).unwrap();
        assert_eq!(converted.status, 502);
        assert_eq!(converted.value["error"]["message"], "overloaded");
        assert!(matches!(
            converted.effects.feedback,
            Some(Feedback::TemporarilyUnavailable(_))
        ));
    }

    #[test]
    fn incomplete_event_becomes_non_streaming_response() {
        let body = br#"data: {"type":"response.incomplete","response":{"id":"resp_incomplete","object":"response","status":"incomplete","incomplete_details":{"reason":"max_output_tokens"},"output":[],"usage":{"input_tokens":3,"output_tokens":5}}}

data: [DONE]

"#;
        let converted = convert_responses_sse_to_json(body, 200).unwrap();
        assert_eq!(converted.status, 200);
        assert_eq!(converted.value["id"], "resp_incomplete");
        assert_eq!(converted.value["status"], "incomplete");
        assert_eq!(
            converted.value["incomplete_details"]["reason"],
            "max_output_tokens"
        );
        assert_eq!(converted.effects.usage.unwrap().total_tokens, 8);
    }
}
