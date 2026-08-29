use std::collections::BTreeMap;

use serde_json::Value;

use gpt_codex_plugin_utils::sse::try_for_each_json_data_value;

use crate::{Effects, response::effects_from_raw_json};

/// 成功的 SSE→JSON 转换结果。`effects` 在任何字段改造前从原始终止事件中提取，确保
/// usage 与 maintenance 不受下游响应瘦身或 output 修复影响。
pub struct ConvertedResponsesBody {
    pub value: Value,
    pub effects: Effects,
}

/// 将完整的 OpenAI Responses SSE body 转成单个非流式 Responses JSON。
///
/// OpenAI 的非流式 Responses API 返回完整的 Response object，而不是终止事件 envelope。
/// 因此这里提取唯一终止事件中的 `response`，并保留 completed、incomplete、failed 等
/// 全部状态。唯一允许的重建是：终止对象的 `output` 缺失或为空时，按 `output_index` 收集
/// 原始 `response.output_item.done.item`。其他字段既不从早期快照补齐，也不生成默认值。
pub fn convert_responses_sse_to_json(
    body: &[u8],
    upstream_status: u16,
) -> Result<Option<ConvertedResponsesBody>, String> {
    let mut terminal_event = None::<Value>;
    let mut terminal_type = None::<String>;
    let mut done_items = Vec::<DoneItem>::new();

    let has_sse_framing = try_for_each_json_data_value(body, |mut event| {
        let event_object = event
            .as_object()
            .ok_or_else(|| "Responses SSE data 必须是 JSON object".to_owned())?;
        let event_type = required_non_empty_string(&event, "type", "SSE event")?.to_owned();
        match event_type.as_str() {
            "response.created" | "response.in_progress" => {
                if terminal_event.is_some() {
                    return Err(format!("终止事件之后不能再出现 {event_type}"));
                }
            }
            "response.output_item.done" => {
                if terminal_event.is_some() {
                    return Err("终止事件之后不能再出现 response.output_item.done".to_owned());
                }
                // done item 只做原值搬运，不校验具体 output item Schema。即使上游返回了新
                // 类型或非标准结构，也交给最终消费方决定是否接受。
                let output_index = parse_output_index(event_object.get("output_index"))?;
                let item = event
                    .as_object_mut()
                    .expect("SSE data 已校验为 JSON object")
                    .remove("item")
                    .ok_or_else(|| "response.output_item.done 缺少 item".to_owned())?;
                done_items.push(DoneItem { output_index, item });
            }
            "response.completed"
            | "response.done"
            | "response.incomplete"
            | "response.failed"
            | "response.cancelled" => {
                if let Some(previous) = terminal_type.as_deref() {
                    return Err(format!(
                        "Responses SSE 包含多个终止事件: {previous} 和 {event_type}"
                    ));
                }
                event_object
                    .get("response")
                    .and_then(Value::as_object)
                    .ok_or_else(|| format!("{event_type} 缺少 response object"))?;
                terminal_type = Some(event_type.to_owned());
                terminal_event = Some(event);
            }
            _ => {}
        }
        Ok(())
    })?;
    if !has_sse_framing {
        return Ok(None);
    }

    let mut terminal = terminal_event.ok_or_else(|| {
        "Responses SSE 缺少 completed、done、incomplete、failed 或 cancelled 终止事件".to_owned()
    })?;
    let effects = effects_from_raw_json(&terminal, Some(upstream_status));
    let response = terminal
        .as_object_mut()
        .expect("终止事件已校验为 JSON object")
        .remove("response")
        .expect("终止事件的 response 已在事件遍历阶段校验");
    let Value::Object(mut response) = response else {
        unreachable!("终止事件的 response 已在事件遍历阶段校验为 JSON object");
    };
    finalize_response(&mut response, done_items)?;
    let response = Value::Object(response);

    Ok(Some(ConvertedResponsesBody {
        value: response,
        effects,
    }))
}

struct DoneItem {
    output_index: Option<usize>,
    item: Value,
}

fn parse_output_index(value: Option<&Value>) -> Result<Option<usize>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    let index = value
        .as_u64()
        .and_then(|index| usize::try_from(index).ok())
        .ok_or_else(|| "response.output_item.done.output_index 必须是非负整数".to_owned())?;
    Ok(Some(index))
}

fn finalize_response(
    response: &mut serde_json::Map<String, Value>,
    done_items: Vec<DoneItem>,
) -> Result<(), String> {
    // 只有字段缺失或确实是空数组时才重建 output。其他值一律原样保留，由下游校验。
    let replace_output = match response.get("output") {
        None => true,
        Some(Value::Array(output)) => output.is_empty(),
        Some(_) => false,
    };
    if replace_output {
        response.insert(
            "output".to_owned(),
            Value::Array(order_done_items(done_items)?),
        );
    }
    Ok(())
}

fn order_done_items(done_items: Vec<DoneItem>) -> Result<Vec<Value>, String> {
    let indexed = done_items
        .iter()
        .filter(|item| item.output_index.is_some())
        .count();
    if indexed == 0 {
        return Ok(done_items.into_iter().map(|item| item.item).collect());
    }
    if indexed != done_items.len() {
        return Err("同一 Responses SSE 不能混用有/无 output_index 的 done item".to_owned());
    }

    let mut ordered = BTreeMap::new();
    for item in done_items {
        let index = item.output_index.expect("all done items are indexed");
        if ordered.insert(index, item.item).is_some() {
            return Err(format!(
                "重复的 response.output_item.done.output_index: {index}"
            ));
        }
    }
    for (expected, actual) in ordered.keys().copied().enumerate() {
        if expected != actual {
            return Err(format!(
                "response.output_item.done.output_index 不连续，期望 {expected}，实际 {actual}"
            ));
        }
    }
    Ok(ordered.into_values().collect())
}

fn required_non_empty_string<'a>(
    value: &'a Value,
    field: &str,
    context: &str,
) -> Result<&'a str, String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("{context}.{field} 必须是非空字符串"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Feedback, Usage};

    fn convert(body: &[u8]) -> ConvertedResponsesBody {
        convert_responses_sse_to_json(body, 200).unwrap().unwrap()
    }

    #[test]
    fn completed_event_becomes_non_streaming_response() {
        let body = br#"data: {"type":"response.created","response":{"id":"resp_1","status":"in_progress","output":[]}}

data: {"type":"response.output_item.done","output_index":0,"item":{"id":"msg_1","type":"message","status":"completed","role":"assistant","content":[{"type":"output_text","text":"hello","annotations":[]}]}}

data: {"type":"response.completed","response":{"id":"resp_1","object":"response","created_at":0,"status":"completed","error":null,"incomplete_details":null,"instructions":null,"metadata":{},"model":"gpt-test","output":[],"parallel_tool_calls":true,"tools":[],"tool_choice":"auto","text":{},"truncation":"disabled","temperature":null,"top_p":null,"usage":{"input_tokens":3,"output_tokens":2,"total_tokens":5}}}

data: [DONE]

"#;
        let converted = convert(body);
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

data: {"type":"response.output_item.done","item":{"id":"fc_1","type":"function_call","status":"completed","call_id":"call_1","name":"lookup","arguments":"{\"q\":1}","future_field":true}}

data: {"type":"response.completed","response":{"id":"resp_2","object":"response","created_at":0,"status":"completed","error":null,"incomplete_details":null,"instructions":null,"metadata":{},"model":"gpt-test","output":[],"parallel_tool_calls":true,"tools":[],"tool_choice":"auto","text":{},"truncation":"disabled","temperature":null,"top_p":null,"usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}}}

"#;
        let converted = convert(body);
        assert_eq!(converted.value["output"].as_array().unwrap().len(), 2);
        assert_eq!(converted.value["output"][0]["encrypted_content"], "opaque");
        assert_eq!(converted.value["output"][1]["future_field"], true);
        assert_eq!(converted.value["output"][1]["arguments"], r#"{"q":1}"#);
    }

    #[test]
    fn deltas_do_not_fabricate_incomplete_output_items() {
        let body = br#"data: {"type":"response.reasoning_summary_text.delta","delta":"think"}

data: {"type":"response.output_text.delta","delta":"hel"}

data: {"type":"response.output_text.delta","delta":"lo"}

data: {"type":"response.output_item.added","output_index":2,"item":{"type":"function_call","call_id":"call_1","name":"lookup"}}

data: {"type":"response.function_call_arguments.delta","output_index":2,"delta":"{\"q\":"}

data: {"type":"response.function_call_arguments.delta","output_index":2,"delta":"1}"}

data: {"type":"response.completed","response":{"id":"resp_3","object":"response","created_at":0,"status":"completed","error":null,"incomplete_details":null,"instructions":null,"metadata":{},"model":"gpt-test","output":[],"parallel_tool_calls":true,"tools":[],"tool_choice":"auto","text":{},"truncation":"disabled","temperature":null,"top_p":null,"usage":{"input_tokens":1,"output_tokens":2,"total_tokens":3}}}

"#;
        let converted = convert(body);
        assert!(converted.value["output"].as_array().unwrap().is_empty());
    }

    #[test]
    fn failed_event_keeps_standard_response_and_feedback() {
        let body = br#"data: {"type":"response.failed","response":{"id":"resp_failed","object":"response","created_at":0,"status":"failed","error":{"code":"server_error","message":"overloaded"},"incomplete_details":null,"instructions":null,"metadata":{},"model":"gpt-test","output":[],"parallel_tool_calls":true,"tools":[],"tool_choice":"auto","text":{},"truncation":"disabled","temperature":null,"top_p":null,"usage":null}}

data: [DONE]

        "#;
        let converted = convert(body);
        assert_eq!(converted.value["status"], "failed");
        assert_eq!(converted.value["error"]["message"], "overloaded");
        assert!(matches!(
            converted.effects.feedback,
            Some(Feedback::TemporarilyUnavailable(_))
        ));
    }

    #[test]
    fn incomplete_event_becomes_non_streaming_response() {
        let body = br#"data: {"type":"response.incomplete","response":{"id":"resp_incomplete","object":"response","created_at":0,"status":"incomplete","error":null,"incomplete_details":{"reason":"max_output_tokens"},"instructions":null,"metadata":{},"model":"gpt-test","output":[],"parallel_tool_calls":true,"tools":[],"tool_choice":"auto","text":{},"truncation":"disabled","temperature":null,"top_p":null,"usage":{"input_tokens":3,"output_tokens":5,"total_tokens":8}}}

data: [DONE]

        "#;
        let converted = convert(body);
        assert_eq!(converted.value["id"], "resp_incomplete");
        assert_eq!(converted.value["status"], "incomplete");
        assert_eq!(
            converted.value["incomplete_details"]["reason"],
            "max_output_tokens"
        );
        assert_eq!(converted.effects.usage.unwrap().total_tokens, 8);
    }
}
