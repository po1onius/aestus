//! OpenAI Images API 与 ChatGPT Codex Images 端点之间的纯协议转换。
//!
//! 首版网关只公开所有 GPT 资源都能稳定执行的 `gpt-image-2` buffered 子集。任何会因
//! Account/API Key 调度结果不同而改变语义的参数都在调度前显式拒绝，不能静默丢弃。

use axum::body::Bytes;
use serde_json::{Map, Value};

use crate::provider::protocol::TokenUsage;

pub(super) const CODEX_IMAGE_MODEL: &str = "gpt-image-2";
const MAX_PROMPT_CHARS: usize = 32_000;

/// 调度前验证调用方请求，并返回用于模型白名单授权和请求日志的有效模型。
pub(super) fn inspect_generations_body(body: &[u8]) -> Result<&'static str, String> {
    build_codex_generations_body(body).map(|_| CODEX_IMAGE_MODEL)
}

/// 把公开的 Images generations 子集转换为 Codex 账号端点接受的 JSON。
pub(super) fn transform_generations_body(body: &[u8]) -> Result<Bytes, String> {
    build_codex_generations_body(body).map(Bytes::from)
}

fn build_codex_generations_body(body: &[u8]) -> Result<Vec<u8>, String> {
    let input: Value = serde_json::from_slice(body)
        .map_err(|error| format!("图片生成请求体不是合法 JSON: {error}"))?;
    let input = input
        .as_object()
        .ok_or_else(|| "图片生成请求体必须是 JSON object".to_owned())?;

    // null 是 OpenAI SDK 常见的“未设置”编码，可以安全忽略；任何非 null 未支持字段都
    // 必须报错，否则混合资源分组可能在 Account 与 API Key 之间产生不同结果。
    const SUPPORTED_FIELDS: [&str; 7] = [
        "prompt",
        "model",
        "background",
        "n",
        "quality",
        "size",
        "stream",
    ];
    if let Some((field, _)) = input
        .iter()
        .find(|(field, value)| !SUPPORTED_FIELDS.contains(&field.as_str()) && !value.is_null())
    {
        return Err(format!(
            "图片生成参数 `{field}` 当前不受网关的 gpt-image-2 buffered 接口支持"
        ));
    }

    let prompt = required_non_empty_string(input, "prompt")?;
    if prompt.chars().count() > MAX_PROMPT_CHARS {
        return Err(format!(
            "图片生成 prompt 不能超过 {MAX_PROMPT_CHARS} 个字符"
        ));
    }
    validate_model(input.get("model"))?;
    validate_stream(input.get("stream"))?;

    let mut output = Map::new();
    output.insert("prompt".to_owned(), Value::String(prompt.to_owned()));
    output.insert(
        "model".to_owned(),
        Value::String(CODEX_IMAGE_MODEL.to_owned()),
    );
    copy_optional_enum(
        input,
        &mut output,
        "background",
        &["transparent", "opaque", "auto"],
    )?;
    copy_optional_image_count(input, &mut output)?;
    copy_optional_enum(
        input,
        &mut output,
        "quality",
        &["low", "medium", "high", "auto"],
    )?;
    copy_optional_non_empty_string(input, &mut output, "size")?;

    serde_json::to_vec(&Value::Object(output))
        .map_err(|error| format!("Codex 图片生成请求序列化失败: {error}"))
}

fn required_non_empty_string<'a>(
    object: &'a Map<String, Value>,
    field: &str,
) -> Result<&'a str, String> {
    object
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("图片生成请求缺少非空字符串 `{field}`"))
}

fn validate_model(value: Option<&Value>) -> Result<(), String> {
    match value {
        None | Some(Value::Null) => Ok(()),
        Some(Value::String(model)) if model.trim() == CODEX_IMAGE_MODEL => Ok(()),
        Some(Value::String(model)) => Err(format!(
            "图片生成当前只支持模型 `{CODEX_IMAGE_MODEL}`，收到 `{}`",
            model.trim()
        )),
        Some(_) => Err("图片生成字段 `model` 必须是字符串或 null".to_owned()),
    }
}

fn validate_stream(value: Option<&Value>) -> Result<(), String> {
    match value {
        None | Some(Value::Null) | Some(Value::Bool(false)) => Ok(()),
        Some(Value::Bool(true)) => {
            Err("图片生成当前只支持 buffered 响应，不能设置 `stream=true`".to_owned())
        }
        Some(_) => Err("图片生成字段 `stream` 必须是 boolean 或 null".to_owned()),
    }
}

fn copy_optional_enum(
    input: &Map<String, Value>,
    output: &mut Map<String, Value>,
    field: &str,
    allowed: &[&str],
) -> Result<(), String> {
    let Some(value) = input.get(field) else {
        return Ok(());
    };
    if value.is_null() {
        return Ok(());
    }
    let value = value
        .as_str()
        .ok_or_else(|| format!("图片生成字段 `{field}` 必须是字符串或 null"))?
        .trim();
    if !allowed.contains(&value) {
        return Err(format!(
            "图片生成字段 `{field}` 的值 `{value}` 不受支持，可选值为 {}",
            allowed.join(", ")
        ));
    }
    output.insert(field.to_owned(), Value::String(value.to_owned()));
    Ok(())
}

fn copy_optional_image_count(
    input: &Map<String, Value>,
    output: &mut Map<String, Value>,
) -> Result<(), String> {
    let Some(value) = input.get("n") else {
        return Ok(());
    };
    if value.is_null() {
        return Ok(());
    }
    let count = value
        .as_u64()
        .filter(|count| (1..=10).contains(count))
        .ok_or_else(|| "图片生成字段 `n` 必须是 1 到 10 之间的整数".to_owned())?;
    output.insert("n".to_owned(), Value::from(count));
    Ok(())
}

fn copy_optional_non_empty_string(
    input: &Map<String, Value>,
    output: &mut Map<String, Value>,
    field: &str,
) -> Result<(), String> {
    let Some(value) = input.get(field) else {
        return Ok(());
    };
    if value.is_null() {
        return Ok(());
    }
    let value = value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("图片生成字段 `{field}` 必须是非空字符串或 null"))?;
    output.insert(field.to_owned(), Value::String(value.to_owned()));
    Ok(())
}

/// 从任意 OpenAI/Codex Images 成功响应中提取 token usage。
pub(super) fn parse_image_usage(body: &[u8]) -> Result<Option<TokenUsage>, String> {
    let value: Value =
        serde_json::from_slice(body).map_err(|error| format!("图片响应不是合法 JSON: {error}"))?;
    parse_usage_value(&value)
}

fn parse_usage_value(value: &Value) -> Result<Option<TokenUsage>, String> {
    let Some(usage) = value.get("usage") else {
        return Ok(None);
    };
    if usage.is_null() {
        return Ok(None);
    }
    let usage = usage
        .as_object()
        .ok_or_else(|| "图片响应 usage 必须是 JSON object 或 null".to_owned())?;
    let input_tokens = required_non_negative_integer(usage, "input_tokens")?;
    let output_tokens = required_non_negative_integer(usage, "output_tokens")?;
    let total_tokens = required_non_negative_integer(usage, "total_tokens")?;
    Ok(Some(TokenUsage {
        input_tokens,
        cached_input_tokens: 0,
        output_tokens,
        reasoning_output_tokens: 0,
        total_tokens,
    }))
}

fn required_non_negative_integer(object: &Map<String, Value>, field: &str) -> Result<i64, String> {
    object
        .get(field)
        .and_then(Value::as_i64)
        .filter(|value| *value >= 0)
        .ok_or_else(|| format!("图片响应 usage.{field} 必须是非负整数"))
}

/// 将 Codex 账号图片响应归一化为 OpenAI Images JSON，同时保留可公开的元数据和 usage。
pub(super) fn transform_account_image_response(body: &[u8]) -> Result<Bytes, String> {
    let value: Value = serde_json::from_slice(body)
        .map_err(|error| format!("Codex 图片响应不是合法 JSON: {error}"))?;
    // usage 在改写前先严格校验，避免把不可扣减的畸形 usage 原样伪装成成功结果。
    let _ = parse_usage_value(&value)?;

    let mut images = direct_b64_images(&value);
    if images.is_empty() {
        collect_input_image_base64(&value, &mut images);
    }
    if images.is_empty() {
        return Err("Codex 图片响应中没有 data[].b64_json 或 input_image base64".to_owned());
    }

    let mut output = Map::new();
    for field in [
        "created",
        "background",
        "output_format",
        "quality",
        "size",
        "usage",
    ] {
        if let Some(field_value) = value.get(field) {
            output.insert(field.to_owned(), field_value.clone());
        }
    }
    output.insert(
        "data".to_owned(),
        Value::Array(
            images
                .into_iter()
                .map(|b64_json| {
                    Value::Object(Map::from_iter([(
                        "b64_json".to_owned(),
                        Value::String(b64_json),
                    )]))
                })
                .collect(),
        ),
    );
    serde_json::to_vec(&Value::Object(output))
        .map(Bytes::from)
        .map_err(|error| format!("OpenAI 图片响应序列化失败: {error}"))
}

fn direct_b64_images(value: &Value) -> Vec<String> {
    value
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item.get("b64_json").and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect()
}

fn collect_input_image_base64(value: &Value, output: &mut Vec<String>) {
    match value {
        Value::Object(object) => {
            if object.get("type").and_then(Value::as_str) == Some("input_image")
                && let Some(encoded) = object
                    .get("image_url")
                    .and_then(Value::as_str)
                    .and_then(base64_payload_from_data_url)
            {
                output.push(encoded.to_owned());
            }
            for child in object.values() {
                collect_input_image_base64(child, output);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_input_image_base64(item, output);
            }
        }
        _ => {}
    }
}

fn base64_payload_from_data_url(url: &str) -> Option<&str> {
    let (metadata, payload) = url.strip_prefix("data:")?.split_once(',')?;
    metadata
        .split(';')
        .any(|part| part.trim().eq_ignore_ascii_case("base64"))
        .then_some(payload.trim())
        .filter(|payload| !payload.is_empty())
}
