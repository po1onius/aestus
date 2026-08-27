//! 请求插件与 buffered 响应插件之间的透明重建上下文。
//!
//! ChatGPT Codex 的 `response.completed.response` 只保证 CLI 实际消费的少数字段，
//! 不能假设它始终回显标准 Responses API 的完整响应配置。因此请求阶段保存最终发往
//! 上游的配置投影，响应阶段仅在 SSE 没有提供对应字段时使用这些值。

use serde_json::{Map, Value, json};

/// 与宿主 `MAX_PLUGIN_CONTEXT_BYTES` 保持一致。非流式请求若无法在这个边界内保存完整的
/// 响应配置投影，应在发送上游前明确拒绝，不能在响应阶段静默返回字段不完整的对象。
const MAX_RESPONSE_CONTEXT_BYTES: usize = 64 * 1024;
const RESPONSE_CONTEXT_VERSION: u64 = 1;

/// 从已经完成 Codex OAuth 归一化的请求构建非流式 Response 的兜底外壳。
pub fn encode_response_context(request: &Value) -> Result<Vec<u8>, String> {
    let request = request
        .as_object()
        .ok_or_else(|| "响应重建上下文要求请求体为 JSON object".to_owned())?;
    let model = request
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .ok_or_else(|| "响应重建上下文缺少非空 model".to_owned())?;

    let mut response = Map::new();
    response.insert("model".to_owned(), Value::String(model.to_owned()));
    response.insert(
        "instructions".to_owned(),
        request.get("instructions").cloned().unwrap_or(Value::Null),
    );
    response.insert("max_output_tokens".to_owned(), Value::Null);
    response.insert(
        "parallel_tool_calls".to_owned(),
        request
            .get("parallel_tool_calls")
            .filter(|value| value.is_boolean())
            .cloned()
            .unwrap_or(Value::Bool(true)),
    );
    response.insert("previous_response_id".to_owned(), Value::Null);
    response.insert(
        "reasoning".to_owned(),
        request.get("reasoning").cloned().unwrap_or(Value::Null),
    );
    response.insert("store".to_owned(), Value::Bool(false));
    response.insert("temperature".to_owned(), Value::Null);
    response.insert(
        "text".to_owned(),
        request
            .get("text")
            .filter(|value| value.is_object())
            .cloned()
            .unwrap_or_else(|| json!({"format": {"type": "text"}})),
    );
    response.insert(
        "tool_choice".to_owned(),
        request
            .get("tool_choice")
            .filter(|value| !value.is_null())
            .cloned()
            .unwrap_or_else(|| Value::String("auto".to_owned())),
    );
    response.insert(
        "tools".to_owned(),
        request
            .get("tools")
            .filter(|value| value.is_array())
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new())),
    );
    response.insert("top_p".to_owned(), Value::Null);
    response.insert(
        "truncation".to_owned(),
        request
            .get("truncation")
            .filter(|value| value.is_string())
            .cloned()
            .unwrap_or_else(|| Value::String("disabled".to_owned())),
    );
    response.insert("user".to_owned(), Value::Null);
    response.insert("metadata".to_owned(), Value::Object(Map::new()));

    // 这些可选字段只有调用方实际提供且上游接受时才应该出现在 Response 中。
    for field in [
        "background",
        "max_tool_calls",
        "prompt_cache_key",
        "service_tier",
        "top_logprobs",
    ] {
        if let Some(value) = request.get(field) {
            response.insert(field.to_owned(), value.clone());
        }
    }

    let encoded = serde_json::to_vec(&json!({
        "version": RESPONSE_CONTEXT_VERSION,
        "response": response,
    }))
    .map_err(|error| format!("响应重建上下文无法序列化: {error}"))?;
    if encoded.len() > MAX_RESPONSE_CONTEXT_BYTES {
        return Err(format!(
            "非流式 Responses 的响应重建上下文超过宿主限制: {} bytes，最大 {} bytes",
            encoded.len(),
            MAX_RESPONSE_CONTEXT_BYTES,
        ));
    }
    Ok(encoded)
}

/// 解码同一次 attempt 的请求上下文。版本不匹配属于插件套件内部错误，不能继续用未知
/// 结构拼装响应。
pub fn decode_response_context(context: Option<&[u8]>) -> Result<Map<String, Value>, String> {
    let Some(context) = context else {
        return Ok(Map::new());
    };
    let value: Value = serde_json::from_slice(context)
        .map_err(|error| format!("响应重建上下文不是合法 JSON: {error}"))?;
    if value.get("version").and_then(Value::as_u64) != Some(RESPONSE_CONTEXT_VERSION) {
        return Err("响应重建上下文版本不受支持".to_owned());
    }
    value
        .get("response")
        .and_then(Value::as_object)
        .cloned()
        .ok_or_else(|| "响应重建上下文缺少 response object".to_owned())
}
