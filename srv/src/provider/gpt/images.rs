//! OpenAI Images API 与 ChatGPT Codex Images 端点之间的纯协议转换。
//!
//! 网关固定使用 `gpt-image-2` buffered 协议，只在调度前限制 `model` 与 `stream`。
//! 其他参数保持原值并交给实际 Account/API Key 上游解释，避免网关字段白名单落后于上游。

use std::convert::Infallible;

use axum::body::Bytes;
use base64::{Engine, engine::general_purpose::STANDARD as BASE64_STANDARD};
use futures_util::stream;
use serde_json::{Map, Value};

use crate::provider::protocol::TokenUsage;

pub(super) const CODEX_IMAGE_MODEL: &str = "gpt-image-2";

/// 调度前验证调用方请求，并返回用于模型白名单授权和请求日志的有效模型。
pub(super) fn inspect_generations_body(body: &[u8]) -> Result<&'static str, String> {
    build_codex_generations_body(body).map(|_| CODEX_IMAGE_MODEL)
}

/// 保留 Images generations 参数并补齐 Codex 账号端点要求的权威协议字段。
pub(super) fn transform_generations_body(body: &[u8]) -> Result<Bytes, String> {
    build_codex_generations_body(body).map(Bytes::from)
}

/// 图片编辑在进入资源调度前完整解析一次 multipart，以便请求格式错误直接按 OpenAI
/// invalid_request 返回，而不是占用账号后才表现成上游故障。返回值固定为授权模型。
pub(super) async fn inspect_edits_body(
    content_type: &str,
    body: Bytes,
) -> Result<&'static str, String> {
    transform_edits_multipart_body(content_type, body)
        .await
        .map(|_| CODEX_IMAGE_MODEL)
}

/// 把调用方 multipart 转成可应用 JSON Merge Patch 的中间结构。
///
/// 图片统一保存为 data URL。Codex Account 与 OpenAI Official 现在都接受相同的
/// `images[].image_url` JSON，因此中间结构也就是两类资源共同的最终 wire schema。
pub(super) async fn transform_edits_multipart_body(
    content_type: &str,
    body: Bytes,
) -> Result<Bytes, String> {
    let boundary = multer::parse_boundary(content_type)
        .map_err(|error| format!("图片编辑请求缺少合法 multipart boundary: {error}"))?;
    let body_stream = stream::once(async move { Result::<Bytes, Infallible>::Ok(body) });
    let mut multipart = multer::Multipart::new(body_stream, boundary);
    let mut images = Vec::new();
    let mut request_fields = Map::new();

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|error| format!("解析图片编辑 multipart 字段失败: {error}"))?
    {
        let name = field.name().unwrap_or_default().to_owned();
        let file_name = field.file_name().map(str::to_owned);
        let field_content_type = field.content_type().map(ToString::to_string);
        let bytes = field
            .bytes()
            .await
            .map_err(|error| format!("读取图片编辑 multipart 字段 `{name}` 失败: {error}"))?;

        if matches!(name.as_str(), "image" | "image[]") {
            let media_type =
                resolve_file_media_type(field_content_type.as_deref(), file_name.as_deref());
            let encoded = BASE64_STANDARD.encode(&bytes);
            images.push(image_url_value(format!(
                "data:{media_type};base64,{encoded}"
            )));
            continue;
        }

        if file_name.is_some() {
            let media_type =
                resolve_file_media_type(field_content_type.as_deref(), file_name.as_deref());
            let encoded = BASE64_STANDARD.encode(&bytes);
            let data_url = format!("data:{media_type};base64,{encoded}");
            // Codex JSON edits 使用与 images 元素相同的 image_url object 表示 mask；
            // 其他未来文件参数没有统一 schema，保留为 data URL 字符串交给上游解释。
            let value = if name == "mask" {
                image_url_value(data_url)
            } else {
                Value::String(data_url)
            };
            insert_multipart_value(&mut request_fields, name, value);
            continue;
        }
        let text = String::from_utf8(bytes.to_vec())
            .map_err(|error| format!("图片编辑字段 `{name}` 不是 UTF-8 文本: {error}"))?;
        let value = match name.as_str() {
            "stream" => Value::Bool(match text.trim() {
                "true" => true,
                "false" => false,
                _ => return Err("图片编辑字段 `stream` 必须是 boolean".to_owned()),
            }),
            // multipart 没有 JSON scalar 类型。对 Images API 当前的数字字段做无损
            // best-effort 转换；非法值保持字符串并交给上游解释，网关不再限制它们。
            "n" | "output_compression" | "partial_images" => text
                .trim()
                .parse::<u64>()
                .map(Value::from)
                .unwrap_or_else(|_| Value::String(text)),
            _ => Value::String(text),
        };
        insert_multipart_value(&mut request_fields, name, value);
    }

    if !images.is_empty() {
        request_fields.insert("images".to_owned(), Value::Array(images));
    }
    normalize_edits_object(&mut request_fields)?;
    serialize_json_object(request_fields, "图片编辑中间请求序列化失败")
}

pub(super) struct FinalizedEditsBody {
    pub body: Bytes,
    pub image_count: usize,
}

/// 资源 override 后重新验证并归一化共同的 Codex/OpenAI Images edits JSON。
pub(super) fn finalize_edits_body(body: &[u8]) -> Result<FinalizedEditsBody, String> {
    let value: Value = serde_json::from_slice(body)
        .map_err(|error| format!("图片编辑中间请求不是合法 JSON: {error}"))?;
    let mut output = value
        .as_object()
        .cloned()
        .ok_or_else(|| "图片编辑中间请求必须是 JSON object".to_owned())?;
    normalize_edits_object(&mut output)?;
    let image_count = output
        .get("images")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let body = serialize_json_object(output, "图片编辑请求序列化失败")?;
    Ok(FinalizedEditsBody { body, image_count })
}

fn serialize_json_object(output: Map<String, Value>, owner: &str) -> Result<Bytes, String> {
    serde_json::to_vec(&Value::Object(output))
        .map(Bytes::from)
        .map_err(|error| format!("{owner}: {error}"))
}

fn normalize_edits_object(output: &mut Map<String, Value>) -> Result<(), String> {
    validate_image_model(output.get("model"), "图片编辑")?;
    validate_image_stream(output.get("stream"), "图片编辑")?;
    output.insert(
        "model".to_owned(),
        Value::String(CODEX_IMAGE_MODEL.to_owned()),
    );
    output.remove("stream");
    Ok(())
}

fn insert_multipart_value(output: &mut Map<String, Value>, name: String, value: Value) {
    match output.remove(&name) {
        None => {
            output.insert(name, value);
        }
        Some(Value::Array(mut values)) => {
            values.push(value);
            output.insert(name, Value::Array(values));
        }
        Some(previous) => {
            output.insert(name, Value::Array(vec![previous, value]));
        }
    }
}

fn image_url_value(image_url: String) -> Value {
    Value::Object(Map::from_iter([(
        "image_url".to_owned(),
        Value::String(image_url),
    )]))
}

fn resolve_file_media_type(content_type: Option<&str>, file_name: Option<&str>) -> String {
    content_type
        .map(str::trim)
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            file_name
                .and_then(|name| mime_guess::from_path(name).first_raw())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "application/octet-stream".to_owned())
}

fn build_codex_generations_body(body: &[u8]) -> Result<Vec<u8>, String> {
    let input: Value = serde_json::from_slice(body)
        .map_err(|error| format!("图片生成请求体不是合法 JSON: {error}"))?;
    let mut output = input
        .as_object()
        .cloned()
        .ok_or_else(|| "图片生成请求体必须是 JSON object".to_owned())?;

    validate_model(output.get("model"))?;
    validate_stream(output.get("stream"))?;
    output.insert(
        "model".to_owned(),
        Value::String(CODEX_IMAGE_MODEL.to_owned()),
    );
    output.remove("stream");

    serde_json::to_vec(&Value::Object(output))
        .map_err(|error| format!("Codex 图片生成请求序列化失败: {error}"))
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

fn validate_image_model(value: Option<&Value>, owner: &str) -> Result<(), String> {
    match value {
        None | Some(Value::Null) => Ok(()),
        Some(Value::String(model)) if model.trim() == CODEX_IMAGE_MODEL => Ok(()),
        Some(Value::String(model)) => Err(format!(
            "{owner}当前只支持模型 `{CODEX_IMAGE_MODEL}`，收到 `{}`",
            model.trim()
        )),
        Some(_) => Err(format!("{owner}字段 `model` 必须是字符串或 null")),
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

fn validate_image_stream(value: Option<&Value>, owner: &str) -> Result<(), String> {
    match value {
        None | Some(Value::Null) | Some(Value::Bool(false)) => Ok(()),
        Some(Value::Bool(true)) => Err(format!(
            "{owner}当前只支持 buffered 响应，不能设置 `stream=true`"
        )),
        Some(_) => Err(format!("{owner}字段 `stream` 必须是 boolean 或 null")),
    }
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
