use std::collections::BTreeSet;

use serde_json::Value;

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
/// OpenAI 的非流式 Responses API 返回完整的 Response object，而不是终止事件 envelope。
/// 因此这里提取首个合法终止事件中的 `response`，并保留 completed、incomplete、failed
/// 等全部状态。只有 `response.output_item.done.item` 才是可直接复用的完整 output item；
/// delta 只是传输片段，不能在缺失 id/status 等协议字段时臆造 output object。
pub fn convert_responses_sse_to_json(
    body: &[u8],
    upstream_status: u16,
) -> Result<Option<ConvertedResponsesBody>, String> {
    if !body_has_sse_framing(body) {
        return Ok(None);
    }

    let mut terminal_event = None::<Value>;
    let mut done_items = Vec::<Value>::new();
    let mut seen_done_items = BTreeSet::<String>::new();
    for_each_json_data_value(body, |event| {
        let event_type = event.get("type").and_then(Value::as_str).map(str::trim);
        match event_type {
            Some(
                "response.completed"
                | "response.done"
                | "response.incomplete"
                | "response.failed"
                | "response.cancelled"
                | "response.canceled",
            ) if terminal_event.is_none() => {
                terminal_event = Some(event);
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

    let mut terminal = terminal_event.ok_or_else(|| {
        "Responses SSE 缺少 completed、done、incomplete、failed 或 cancelled 终止事件".to_owned()
    })?;
    let effects = effects_from_raw_json(&terminal, Some(upstream_status), false);
    let mut response = terminal
        .get_mut("response")
        .filter(|response| response.is_object())
        .map(Value::take)
        .ok_or_else(|| "Responses SSE 终止事件缺少 response object".to_owned())?;

    // 某些 Codex 流会在终止 Response 中给出空 output，但此前已经发送完整的 done item。
    // 此时按到达顺序去重后补回原始对象，未知字段也原样保留，避免协议信息损失。
    if response_output_is_empty(&response)
        && !done_items.is_empty()
        && let Some(response) = response.as_object_mut()
    {
        response.insert("output".to_owned(), Value::Array(done_items));
    }
    validate_non_streaming_response(&response)?;

    Ok(Some(ConvertedResponsesBody {
        status: upstream_status,
        value: response,
        effects,
    }))
}

fn response_output_is_empty(response: &Value) -> bool {
    response
        .get("output")
        .and_then(Value::as_array)
        .is_none_or(Vec::is_empty)
}

/// 校验非流式成功响应的 Responses 核心结构。
///
/// output item 类型会持续扩展，因此这里只验证所有版本都稳定存在的公共字段，不在插件
/// 中复制整份 OpenAI schema。这样既能阻止 SSE envelope、错误页或残缺拼装结果冒充
/// Response，又不会因上游新增内建工具类型而误拒绝合法响应。
pub fn validate_non_streaming_response(value: &Value) -> Result<(), String> {
    let response = value
        .as_object()
        .ok_or_else(|| "非流式 Responses 响应必须是 JSON object".to_owned())?;
    required_non_empty_string(value, "id", "response")?;
    if required_non_empty_string(value, "object", "response")? != "response" {
        return Err("response.object 必须为 response".to_owned());
    }
    let status = required_non_empty_string(value, "status", "response")?;
    if !matches!(
        status,
        "completed" | "incomplete" | "failed" | "in_progress" | "cancelled" | "queued"
    ) {
        return Err(format!("response.status 非法: {status}"));
    }

    let output = response
        .get("output")
        .and_then(Value::as_array)
        .ok_or_else(|| "response.output 必须是 array".to_owned())?;
    for (index, item) in output.iter().enumerate() {
        required_non_empty_string(item, "type", &format!("response.output[{index}]"))?;
    }

    if let Some(usage) = response.get("usage").filter(|usage| !usage.is_null()) {
        let usage = usage
            .as_object()
            .ok_or_else(|| "response.usage 必须是 object 或 null".to_owned())?;
        for field in ["input_tokens", "output_tokens", "total_tokens"] {
            if let Some(value) = usage.get(field)
                && value.as_i64().is_none_or(|value| value < 0)
            {
                return Err(format!("response.usage.{field} 必须是非负整数"));
            }
        }
    }
    Ok(())
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

    #[test]
    fn completed_event_becomes_non_streaming_response() {
        let body = br#"data: {"type":"response.completed","response":{"id":"resp_1","object":"response","status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"hello"}]}],"usage":{"input_tokens":3,"output_tokens":2}}}

data: [DONE]

"#;
        let converted = convert_responses_sse_to_json(body, 200).unwrap().unwrap();
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

data: {"type":"response.completed","response":{"id":"resp_2","object":"response","status":"completed","output":[],"usage":{"input_tokens":1,"output_tokens":1}}}

"#;
        let converted = convert_responses_sse_to_json(body, 200).unwrap().unwrap();
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

data: {"type":"response.completed","response":{"id":"resp_3","object":"response","status":"completed","output":[],"usage":{"input_tokens":1,"output_tokens":2}}}

"#;
        let converted = convert_responses_sse_to_json(body, 200).unwrap().unwrap();
        assert!(converted.value["output"].as_array().unwrap().is_empty());
    }

    #[test]
    fn failed_event_keeps_standard_response_and_feedback() {
        let body = br#"data: {"type":"response.failed","response":{"id":"resp_failed","object":"response","status":"failed","error":{"code":"server_error","message":"overloaded"},"output":[]}}

data: [DONE]

"#;
        let converted = convert_responses_sse_to_json(body, 200).unwrap().unwrap();
        assert_eq!(converted.status, 200);
        assert_eq!(converted.value["status"], "failed");
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
        let converted = convert_responses_sse_to_json(body, 200).unwrap().unwrap();
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
