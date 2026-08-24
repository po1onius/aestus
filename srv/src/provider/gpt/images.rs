//! OpenAI Images API 与 ChatGPT Codex Images 端点之间的纯协议转换。
//!
//! 网关只公开所有 GPT 资源都能稳定执行的 `gpt-image-2` buffered 子集。任何会因
//! Account/API Key 调度结果不同而改变语义的参数都在调度前显式拒绝，不能静默丢弃。

use std::convert::Infallible;

use axum::body::Bytes;
use base64::{Engine, engine::general_purpose::STANDARD as BASE64_STANDARD};
use futures_util::stream;
use serde_json::{Map, Value};

use crate::provider::protocol::TokenUsage;

pub(super) const CODEX_IMAGE_MODEL: &str = "gpt-image-2";
const MAX_PROMPT_CHARS: usize = 32_000;
const MAX_EDIT_IMAGES: usize = 16;
const MAX_EDIT_IMAGE_BYTES: usize = 50 * 1024 * 1024;

/// 调度前验证调用方请求，并返回用于模型白名单授权和请求日志的有效模型。
pub(super) fn inspect_generations_body(body: &[u8]) -> Result<&'static str, String> {
    build_codex_generations_body(body).map(|_| CODEX_IMAGE_MODEL)
}

/// 把公开的 Images generations 子集转换为 Codex 账号端点接受的 JSON。
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
    let mut text_fields = Map::new();

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
            if bytes.is_empty() {
                return Err("图片编辑请求中的 image 文件不能为空".to_owned());
            }
            if bytes.len() >= MAX_EDIT_IMAGE_BYTES {
                return Err(format!(
                    "图片编辑 image 文件必须小于 {} MiB",
                    MAX_EDIT_IMAGE_BYTES / 1024 / 1024
                ));
            }
            if images.len() >= MAX_EDIT_IMAGES {
                return Err(format!(
                    "图片编辑请求最多支持 {MAX_EDIT_IMAGES} 个 image 文件"
                ));
            }
            let media_type =
                resolve_image_media_type(field_content_type.as_deref(), file_name.as_deref())?;
            let encoded = BASE64_STANDARD.encode(&bytes);
            images.push(Value::Object(Map::from_iter([(
                "image_url".to_owned(),
                Value::String(format!("data:{media_type};base64,{encoded}")),
            )])));
            continue;
        }

        const TEXT_FIELDS: [&str; 7] = [
            "prompt",
            "model",
            "background",
            "n",
            "quality",
            "size",
            "stream",
        ];
        if !TEXT_FIELDS.contains(&name.as_str()) {
            return Err(format!(
                "图片编辑参数 `{name}` 当前不受网关的 gpt-image-2 buffered 接口支持"
            ));
        }
        if file_name.is_some() {
            return Err(format!("图片编辑字段 `{name}` 必须是文本字段"));
        }
        if text_fields.contains_key(&name) {
            return Err(format!("图片编辑字段 `{name}` 不能重复提交"));
        }
        let text = String::from_utf8(bytes.to_vec())
            .map_err(|error| format!("图片编辑字段 `{name}` 不是 UTF-8 文本: {error}"))?;
        let value = match name.as_str() {
            "n" => Value::from(
                text.trim()
                    .parse::<u64>()
                    .map_err(|_| "图片编辑字段 `n` 必须是整数".to_owned())?,
            ),
            "stream" => Value::Bool(match text.trim() {
                "true" => true,
                "false" => false,
                _ => return Err("图片编辑字段 `stream` 必须是 boolean".to_owned()),
            }),
            _ => Value::String(text),
        };
        text_fields.insert(name, value);
    }

    text_fields.insert("images".to_owned(), Value::Array(images));
    let normalized = normalize_edits_value(&Value::Object(text_fields))?;
    serialize_edits_intermediate(&normalized)
}

pub(super) struct FinalizedEditsBody {
    pub body: Bytes,
    pub image_count: usize,
}

/// 资源 override 后重新验证并归一化共同的 Codex/OpenAI Images edits JSON。
pub(super) fn finalize_edits_body(body: &[u8]) -> Result<FinalizedEditsBody, String> {
    let value: Value = serde_json::from_slice(body)
        .map_err(|error| format!("图片编辑中间请求不是合法 JSON: {error}"))?;
    let normalized = normalize_edits_value(&value)?;
    let mut output = common_edits_fields(&normalized);
    output.insert(
        "images".to_owned(),
        Value::Array(
            normalized
                .images
                .iter()
                .map(|image_url| image_json_value(image_url))
                .collect(),
        ),
    );
    let body = serde_json::to_vec(&Value::Object(output))
        .map(Bytes::from)
        .map_err(|error| format!("图片编辑请求序列化失败: {error}"))?;
    Ok(FinalizedEditsBody {
        body,
        image_count: normalized.images.len(),
    })
}

#[derive(Debug)]
struct NormalizedEditRequest {
    images: Vec<String>,
    prompt: String,
    background: Option<String>,
    n: Option<u64>,
    quality: Option<String>,
    size: Option<String>,
}

fn normalize_edits_value(value: &Value) -> Result<NormalizedEditRequest, String> {
    let input = value
        .as_object()
        .ok_or_else(|| "图片编辑中间请求必须是 JSON object".to_owned())?;
    const SUPPORTED_FIELDS: [&str; 8] = [
        "images",
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
            "图片编辑参数 `{field}` 当前不受网关的 gpt-image-2 buffered 接口支持"
        ));
    }

    let prompt = required_non_empty_string_for(input, "prompt", "图片编辑请求")?;
    if prompt.chars().count() > MAX_PROMPT_CHARS {
        return Err(format!(
            "图片编辑 prompt 不能超过 {MAX_PROMPT_CHARS} 个字符"
        ));
    }
    validate_image_model(input.get("model"), "图片编辑")?;
    validate_image_stream(input.get("stream"), "图片编辑")?;

    let image_values = input
        .get("images")
        .and_then(Value::as_array)
        .filter(|images| !images.is_empty())
        .ok_or_else(|| "图片编辑请求至少需要一个 image 文件".to_owned())?;
    if image_values.len() > MAX_EDIT_IMAGES {
        return Err(format!(
            "图片编辑请求最多支持 {MAX_EDIT_IMAGES} 个 image 文件"
        ));
    }
    let mut images = Vec::with_capacity(image_values.len());
    for (index, image) in image_values.iter().enumerate() {
        let image = image
            .as_object()
            .ok_or_else(|| format!("图片编辑 images[{index}] 必须是 JSON object"))?;
        const SUPPORTED_IMAGE_FIELDS: [&str; 1] = ["image_url"];
        if let Some((field, _)) = image.iter().find(|(field, value)| {
            !SUPPORTED_IMAGE_FIELDS.contains(&field.as_str()) && !value.is_null()
        }) {
            return Err(format!(
                "图片编辑 images[{index}].{field} 不受上游中间协议支持"
            ));
        }
        let image_url = image
            .get("image_url")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| format!("图片编辑 images[{index}].image_url 不能为空"))?;
        let (_, payload) = parse_image_data_url(image_url)?;
        let decoded_bytes = BASE64_STANDARD
            .decode(payload)
            .map_err(|error| {
                format!("图片编辑 images[{index}].image_url 不是合法 base64: {error}")
            })?
            .len();
        if decoded_bytes >= MAX_EDIT_IMAGE_BYTES {
            return Err(format!(
                "图片编辑 images[{index}] 必须小于 {} MiB",
                MAX_EDIT_IMAGE_BYTES / 1024 / 1024
            ));
        }
        images.push(image_url.to_owned());
    }

    Ok(NormalizedEditRequest {
        images,
        prompt: prompt.to_owned(),
        background: optional_enum_value(
            input,
            "background",
            &["transparent", "opaque", "auto"],
            "图片编辑",
        )?,
        n: optional_image_count(input, "图片编辑")?,
        quality: optional_enum_value(
            input,
            "quality",
            &["low", "medium", "high", "auto"],
            "图片编辑",
        )?,
        size: optional_non_empty_string_value(input, "size", "图片编辑")?,
    })
}

fn serialize_edits_intermediate(request: &NormalizedEditRequest) -> Result<Bytes, String> {
    let mut output = common_edits_fields(request);
    output.insert(
        "images".to_owned(),
        Value::Array(
            request
                .images
                .iter()
                .map(|image_url| image_json_value(image_url))
                .collect(),
        ),
    );
    serde_json::to_vec(&Value::Object(output))
        .map(Bytes::from)
        .map_err(|error| format!("图片编辑中间请求序列化失败: {error}"))
}

fn image_json_value(image_url: &str) -> Value {
    Value::Object(Map::from_iter([(
        "image_url".to_owned(),
        Value::String(image_url.to_owned()),
    )]))
}

fn common_edits_fields(request: &NormalizedEditRequest) -> Map<String, Value> {
    let mut output = Map::new();
    output.insert("prompt".to_owned(), Value::String(request.prompt.clone()));
    output.insert(
        "model".to_owned(),
        Value::String(CODEX_IMAGE_MODEL.to_owned()),
    );
    if let Some(value) = &request.background {
        output.insert("background".to_owned(), Value::String(value.clone()));
    }
    if let Some(value) = request.n {
        output.insert("n".to_owned(), Value::from(value));
    }
    if let Some(value) = &request.quality {
        output.insert("quality".to_owned(), Value::String(value.clone()));
    }
    if let Some(value) = &request.size {
        output.insert("size".to_owned(), Value::String(value.clone()));
    }
    output
}

fn resolve_image_media_type(
    content_type: Option<&str>,
    file_name: Option<&str>,
) -> Result<String, String> {
    let media_type = content_type
        .map(str::trim)
        .filter(|value| value.starts_with("image/"))
        .map(str::to_owned)
        .or_else(|| {
            file_name
                .and_then(|name| mime_guess::from_path(name).first_raw())
                .filter(|value| value.starts_with("image/"))
                .map(str::to_owned)
        })
        .ok_or_else(|| "图片编辑 image 文件缺少可识别的 image/* 媒体类型".to_owned())?;
    match media_type.split(';').next().map(str::trim) {
        Some("image/png" | "image/jpeg" | "image/webp") => Ok(media_type),
        _ => Err("图片编辑 image 文件只支持 PNG、JPEG 或 WebP".to_owned()),
    }
}

fn parse_image_data_url(url: &str) -> Result<(&str, &str), String> {
    let (metadata, payload) = url
        .strip_prefix("data:")
        .and_then(|url| url.split_once(','))
        .ok_or_else(|| "图片编辑 image_url 必须是 base64 data URL".to_owned())?;
    let mut parts = metadata.split(';');
    let media_type = parts.next().unwrap_or_default().trim();
    if !matches!(media_type, "image/png" | "image/jpeg" | "image/webp")
        || !parts.any(|part| part.eq_ignore_ascii_case("base64"))
    {
        return Err("图片编辑 image_url 必须是 PNG、JPEG 或 WebP 的 base64 data URL".to_owned());
    }
    let payload = payload.trim();
    if payload.is_empty() {
        return Err("图片编辑 image_url 的 base64 数据不能为空".to_owned());
    }
    Ok((media_type, payload))
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

fn required_non_empty_string_for<'a>(
    object: &'a Map<String, Value>,
    field: &str,
    owner: &str,
) -> Result<&'a str, String> {
    object
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("{owner}缺少非空字符串 `{field}`"))
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

fn optional_enum_value(
    input: &Map<String, Value>,
    field: &str,
    allowed: &[&str],
    owner: &str,
) -> Result<Option<String>, String> {
    let Some(value) = input.get(field) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let value = value
        .as_str()
        .ok_or_else(|| format!("{owner}字段 `{field}` 必须是字符串或 null"))?
        .trim();
    if !allowed.contains(&value) {
        return Err(format!(
            "{owner}字段 `{field}` 的值 `{value}` 不受支持，可选值为 {}",
            allowed.join(", ")
        ));
    }
    Ok(Some(value.to_owned()))
}

fn optional_image_count(input: &Map<String, Value>, owner: &str) -> Result<Option<u64>, String> {
    let Some(value) = input.get("n") else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    value
        .as_u64()
        .filter(|count| (1..=10).contains(count))
        .map(Some)
        .ok_or_else(|| format!("{owner}字段 `n` 必须是 1 到 10 之间的整数"))
}

fn optional_non_empty_string_value(
    input: &Map<String, Value>,
    field: &str,
    owner: &str,
) -> Result<Option<String>, String> {
    let Some(value) = input.get(field) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .map(Some)
        .ok_or_else(|| format!("{owner}字段 `{field}` 必须是非空字符串或 null"))
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
