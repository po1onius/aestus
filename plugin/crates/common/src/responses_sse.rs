use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

use crate::{
    Effects, response::effects_from_raw_json, response_context::decode_response_context,
    sse::parse_json_data_values,
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
    request_context: Option<&[u8]>,
    response_date_unix_seconds: Option<i64>,
) -> Result<Option<ConvertedResponsesBody>, String> {
    let Some(events) = parse_json_data_values(body)? else {
        return Ok(None);
    };
    let mut response = decode_response_context(request_context)?;
    let mut terminal_event = None::<Value>;
    let mut terminal_type = None::<String>;
    let mut terminal_status_present = false;
    let mut done_items = Vec::<DoneItem>::new();
    let mut seen_done_identities = BTreeMap::<String, (Option<usize>, Value)>::new();

    for event in events {
        let event_object = event
            .as_object()
            .ok_or_else(|| "Responses SSE data 必须是 JSON object".to_owned())?;
        let event_type = required_non_empty_string(&event, "type", "SSE event")?;
        match event_type {
            "response.created" | "response.in_progress" => {
                if terminal_event.is_some() {
                    return Err(format!("终止事件之后不能再出现 {event_type}"));
                }
                let base = event_object
                    .get("response")
                    .and_then(Value::as_object)
                    .ok_or_else(|| format!("{event_type} 缺少 response object"))?;
                merge_response_fields(&mut response, base);
            }
            "response.output_item.done" => {
                if terminal_event.is_some() {
                    return Err("终止事件之后不能再出现 response.output_item.done".to_owned());
                }
                let mut item = event_object
                    .get("item")
                    .and_then(Value::as_object)
                    .cloned()
                    .ok_or_else(|| "response.output_item.done 缺少 item object".to_owned())?;
                normalize_done_item(&mut item)?;
                let item = Value::Object(item);
                let output_index = parse_output_index(event_object.get("output_index"))?;
                if let Some(identity) = output_item_identity(&item) {
                    if let Some((previous_index, previous_item)) =
                        seen_done_identities.get(&identity)
                    {
                        if previous_index != &output_index || previous_item != &item {
                            return Err(format!("同一 output item 标识出现冲突内容: {identity}"));
                        }
                        continue;
                    }
                    seen_done_identities.insert(identity, (output_index, item.clone()));
                }
                done_items.push(DoneItem { output_index, item });
            }
            "response.completed"
            | "response.done"
            | "response.incomplete"
            | "response.failed"
            | "response.cancelled"
            | "response.canceled" => {
                if let Some(previous) = terminal_type.as_deref() {
                    return Err(format!(
                        "Responses SSE 包含多个终止事件: {previous} 和 {event_type}"
                    ));
                }
                let terminal_response = event_object
                    .get("response")
                    .and_then(Value::as_object)
                    .ok_or_else(|| format!("{event_type} 缺少 response object"))?;
                terminal_status_present = terminal_response.contains_key("status");
                merge_response_fields(&mut response, terminal_response);
                terminal_type = Some(event_type.to_owned());
                terminal_event = Some(event);
            }
            _ => {}
        }
    }

    let terminal = terminal_event.ok_or_else(|| {
        "Responses SSE 缺少 completed、done、incomplete、failed 或 cancelled 终止事件".to_owned()
    })?;
    let effects = effects_from_raw_json(&terminal, Some(upstream_status), false);
    finalize_response(
        &mut response,
        terminal_type.as_deref().expect("terminal type must exist"),
        terminal_status_present,
        done_items,
        response_date_unix_seconds,
    )?;
    let response = Value::Object(response);
    validate_non_streaming_response(&response)?;

    Ok(Some(ConvertedResponsesBody {
        status: upstream_status,
        value: response,
        effects,
    }))
}

struct DoneItem {
    output_index: Option<usize>,
    item: Value,
}

fn merge_response_fields(target: &mut Map<String, Value>, source: &Map<String, Value>) {
    target.extend(
        source
            .iter()
            .map(|(key, value)| (key.clone(), value.clone())),
    );
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

fn output_item_identity(item: &Value) -> Option<String> {
    let kind = item.get("type").and_then(Value::as_str)?;
    item.get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(|id| format!("{kind}:id:{id}"))
        .or_else(|| {
            item.get("call_id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|call_id| !call_id.is_empty())
                .map(|call_id| format!("{kind}:call_id:{call_id}"))
        })
}

fn finalize_response(
    response: &mut Map<String, Value>,
    terminal_type: &str,
    terminal_status_present: bool,
    done_items: Vec<DoneItem>,
    response_date_unix_seconds: Option<i64>,
) -> Result<(), String> {
    if let Some(object) = response.get("object") {
        if object.as_str() != Some("response") {
            return Err("response.object 必须为 response".to_owned());
        }
    } else {
        response.insert("object".to_owned(), Value::String("response".to_owned()));
    }

    let expected_status = match terminal_type {
        "response.completed" => Some("completed"),
        "response.incomplete" => Some("incomplete"),
        "response.failed" => Some("failed"),
        "response.cancelled" | "response.canceled" => Some("cancelled"),
        "response.done" => None,
        _ => return Err(format!("未知终止事件: {terminal_type}")),
    };
    if let Some(expected_status) = expected_status {
        if terminal_status_present
            && let Some(status) = response.get("status")
            && status.as_str() != Some(expected_status)
        {
            return Err(format!(
                "{terminal_type} 与 response.status 不一致，期望 {expected_status}"
            ));
        }
        if !terminal_status_present {
            response.insert(
                "status".to_owned(),
                Value::String(expected_status.to_owned()),
            );
        }
    } else if !terminal_status_present {
        response.insert("status".to_owned(), Value::String("completed".to_owned()));
    }

    let replace_output = match response.get("output") {
        None => true,
        Some(Value::Array(output)) => output.is_empty(),
        Some(_) => return Err("response.output 必须是 array".to_owned()),
    };
    if replace_output {
        response.insert(
            "output".to_owned(),
            Value::Array(order_done_items(done_items)?),
        );
    }
    let response_id = response
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let output = response
        .get_mut("output")
        .and_then(Value::as_array_mut)
        .expect("output was validated as array");
    for (output_index, item) in output.iter_mut().enumerate() {
        let item = item
            .as_object_mut()
            .ok_or_else(|| "response.output item 必须是 object".to_owned())?;
        normalize_done_item(item)?;
        if let Some(response_id) = response_id.as_deref() {
            ensure_output_item_id(item, response_id, output_index);
        }
    }

    if !response.contains_key("created_at") {
        let created_at = response_date_unix_seconds.ok_or_else(|| {
            "精简 Codex SSE 缺少 response.created_at，且上游没有合法 Date header".to_owned()
        })?;
        response.insert("created_at".to_owned(), Value::from(created_at));
    }
    if response.get("status").and_then(Value::as_str) == Some("completed")
        && !response.contains_key("completed_at")
        && let Some(completed_at) = response_date_unix_seconds
    {
        response.insert("completed_at".to_owned(), Value::from(completed_at));
    }

    response.entry("error").or_insert(Value::Null);
    response.entry("incomplete_details").or_insert(Value::Null);
    response.entry("usage").or_insert(Value::Null);
    normalize_usage(response)?;
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

fn normalize_done_item(item: &mut Map<String, Value>) -> Result<(), String> {
    let item_type = item
        .get("type")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|item_type| !item_type.is_empty())
        .ok_or_else(|| "response.output item.type 必须是非空字符串".to_owned())?
        .to_owned();
    if matches!(
        item_type.as_str(),
        "message"
            | "function_call"
            | "custom_tool_call"
            | "web_search_call"
            | "file_search_call"
            | "computer_call"
            | "image_generation_call"
            | "code_interpreter_call"
            | "local_shell_call"
            | "tool_search_call"
    ) {
        item.entry("status")
            .or_insert_with(|| Value::String("completed".to_owned()));
    }
    if item_type == "message" {
        let content = item
            .get_mut("content")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| "message output item.content 必须是 array".to_owned())?;
        for part in content {
            let part = part
                .as_object_mut()
                .ok_or_else(|| "message content part 必须是 object".to_owned())?;
            if part.get("type").and_then(Value::as_str) == Some("output_text") {
                part.entry("annotations")
                    .or_insert_with(|| Value::Array(Vec::new()));
            }
        }
    }
    Ok(())
}

/// Codex CLI 的 ResponseItem 为了兼容多种 Provider，把 message/reasoning 的 id 视为可选；
/// 标准非流式 Responses 输出则要求这两类 item 带 id。仅在上游确实缺失时，根据 response
/// id 与 output_index 生成稳定且不泄漏内容的代理 id，不修改任何模型语义字段。
fn ensure_output_item_id(item: &mut Map<String, Value>, response_id: &str, output_index: usize) {
    if item
        .get("id")
        .and_then(Value::as_str)
        .is_some_and(|id| !id.trim().is_empty())
    {
        return;
    }
    let prefix = match item.get("type").and_then(Value::as_str) {
        Some("message") => "msg",
        Some("reasoning") => "rs",
        _ => return,
    };
    let suffix = response_id
        .strip_prefix("resp_")
        .unwrap_or(response_id)
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || *character == '_')
        // `usize` 十进制最多 20 位；32 字符后缀可确保最终代理 id 不超过 64 字符。
        .take(32)
        .collect::<String>();
    let suffix = if suffix.is_empty() {
        "response"
    } else {
        &suffix
    };
    item.insert(
        "id".to_owned(),
        Value::String(format!("{prefix}_aestus_{suffix}_{output_index}")),
    );
}

fn normalize_usage(response: &mut Map<String, Value>) -> Result<(), String> {
    let Some(usage) = response.get_mut("usage") else {
        return Ok(());
    };
    if usage.is_null() {
        return Ok(());
    }
    let usage = usage
        .as_object_mut()
        .ok_or_else(|| "response.usage 必须是 object 或 null".to_owned())?;
    let input_tokens = non_negative_integer(usage.get("input_tokens"), "usage.input_tokens")?;
    let output_tokens = non_negative_integer(usage.get("output_tokens"), "usage.output_tokens")?;
    usage
        .entry("total_tokens")
        .or_insert(Value::from(input_tokens.saturating_add(output_tokens)));
    usage
        .entry("input_tokens_details")
        .or_insert_with(|| json!({"cached_tokens": 0}));
    usage
        .entry("output_tokens_details")
        .or_insert_with(|| json!({"reasoning_tokens": 0}));
    Ok(())
}

fn non_negative_integer(value: Option<&Value>, field: &str) -> Result<i64, String> {
    value
        .and_then(Value::as_i64)
        .filter(|value| *value >= 0)
        .ok_or_else(|| format!("response.{field} 必须是非负整数"))
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
    non_negative_integer(response.get("created_at"), "created_at")?;
    required_non_empty_string(value, "model", "response")?;
    let status = required_non_empty_string(value, "status", "response")?;
    if !matches!(
        status,
        "completed" | "incomplete" | "failed" | "in_progress" | "cancelled" | "queued"
    ) {
        return Err(format!("response.status 非法: {status}"));
    }
    for field in [
        "error",
        "incomplete_details",
        "instructions",
        "metadata",
        "usage",
    ] {
        if !response.contains_key(field) {
            return Err(format!("response.{field} 必须存在，可为空值"));
        }
    }
    if status == "failed" && !response.get("error").is_some_and(Value::is_object) {
        return Err("failed response.error 必须是 object".to_owned());
    }
    if status == "incomplete"
        && !response
            .get("incomplete_details")
            .is_some_and(Value::is_object)
    {
        return Err("incomplete response.incomplete_details 必须是 object".to_owned());
    }
    if !response
        .get("parallel_tool_calls")
        .is_some_and(Value::is_boolean)
    {
        return Err("response.parallel_tool_calls 必须是 boolean".to_owned());
    }
    if !response.get("tools").is_some_and(Value::is_array) {
        return Err("response.tools 必须是 array".to_owned());
    }
    if !response.contains_key("tool_choice") {
        return Err("response.tool_choice 必须存在".to_owned());
    }
    if !response.get("text").is_some_and(Value::is_object) {
        return Err("response.text 必须是 object".to_owned());
    }
    required_non_empty_string(value, "truncation", "response")?;
    for field in ["temperature", "top_p"] {
        if !response
            .get(field)
            .is_some_and(|value| value.is_null() || value.is_number())
        {
            return Err(format!("response.{field} 必须是 number 或 null"));
        }
    }

    let output = response
        .get("output")
        .and_then(Value::as_array)
        .ok_or_else(|| "response.output 必须是 array".to_owned())?;
    for (index, item) in output.iter().enumerate() {
        let item_type =
            required_non_empty_string(item, "type", &format!("response.output[{index}]"))?;
        if item_type == "message" {
            required_non_empty_string(item, "id", &format!("response.output[{index}]"))?;
            let item_status =
                required_non_empty_string(item, "status", &format!("response.output[{index}]"))?;
            if !matches!(item_status, "in_progress" | "completed" | "incomplete") {
                return Err(format!(
                    "response.output[{index}].status 非法: {item_status}"
                ));
            }
            if required_non_empty_string(item, "role", &format!("response.output[{index}]"))?
                != "assistant"
            {
                return Err(format!("response.output[{index}].role 必须为 assistant"));
            }
            let content = item
                .get("content")
                .and_then(Value::as_array)
                .ok_or_else(|| format!("response.output[{index}].content 必须是 array"))?;
            for (content_index, part) in content.iter().enumerate() {
                let part_type = required_non_empty_string(
                    part,
                    "type",
                    &format!("response.output[{index}].content[{content_index}]"),
                )?;
                if part_type == "output_text" {
                    if part.get("text").and_then(Value::as_str).is_none() {
                        return Err(format!(
                            "response.output[{index}].content[{content_index}].text 必须是 string"
                        ));
                    }
                    if !part.get("annotations").is_some_and(Value::is_array) {
                        return Err(format!(
                            "response.output[{index}].content[{content_index}].annotations 必须是 array"
                        ));
                    }
                }
            }
        }
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
    use crate::{Feedback, Usage, response_context::encode_response_context};

    fn convert(body: &[u8]) -> ConvertedResponsesBody {
        let context = encode_response_context(&json!({"model": "gpt-test"})).unwrap();
        convert_responses_sse_to_json(body, 200, Some(&context), Some(0))
            .unwrap()
            .unwrap()
    }

    #[test]
    fn completed_event_becomes_non_streaming_response() {
        let body = br#"data: {"type":"response.created","response":{"id":"resp_1","status":"in_progress","output":[]}}

data: {"type":"response.output_item.done","output_index":0,"item":{"id":"msg_1","type":"message","role":"assistant","content":[{"type":"output_text","text":"hello"}]}}

data: {"type":"response.completed","response":{"id":"resp_1","usage":{"input_tokens":3,"output_tokens":2}}}

data: [DONE]

"#;
        let converted = convert(body);
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

data: {"type":"response.completed","response":{"id":"resp_3","object":"response","status":"completed","output":[],"usage":{"input_tokens":1,"output_tokens":2}}}

"#;
        let converted = convert(body);
        assert!(converted.value["output"].as_array().unwrap().is_empty());
    }

    #[test]
    fn failed_event_keeps_standard_response_and_feedback() {
        let body = br#"data: {"type":"response.failed","response":{"id":"resp_failed","object":"response","status":"failed","error":{"code":"server_error","message":"overloaded"},"output":[]}}

data: [DONE]

"#;
        let converted = convert(body);
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
        let converted = convert(body);
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
