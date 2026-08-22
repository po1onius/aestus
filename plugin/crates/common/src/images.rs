//! OpenAI Images API 与 ChatGPT Codex Images 端点之间的纯协议转换。
//!
//! generations 的下游请求是 JSON；edits 的下游请求是标准 multipart/form-data。
//! Codex 两个上游端点都接收 JSON，因此编辑请求中的文件必须转换为 data URL。

use std::convert::Infallible;

use base64::{Engine, engine::general_purpose::STANDARD as BASE64_STANDARD};
use futures_util::stream;
use serde_json::{Map, Value};

const CODEX_IMAGE_MODEL: &str = "gpt-image-2";

/// 把标准 `/images/generations` JSON 缩减为 Codex generations 接受的字段集合。
pub fn transform_generations_body(body: &[u8]) -> Result<Vec<u8>, String> {
    let input: Value = serde_json::from_slice(body)
        .map_err(|error| format!("图片生成请求体不是合法 JSON: {error}"))?;
    let input = input
        .as_object()
        .ok_or_else(|| "图片生成请求体必须是 JSON object".to_owned())?;
    let prompt = required_non_empty_string(input, "prompt", "图片生成请求")?;

    let mut output = Map::new();
    output.insert("prompt".to_owned(), Value::String(prompt.to_owned()));
    // Codex 当前图片扩展固定使用 gpt-image-2；下游传入的其他 model 不进入上游。
    output.insert(
        "model".to_owned(),
        Value::String(CODEX_IMAGE_MODEL.to_owned()),
    );
    copy_enum(
        input,
        &mut output,
        "background",
        &["transparent", "opaque", "auto"],
    );
    copy_positive_integer(input, &mut output, "n");
    copy_enum(
        input,
        &mut output,
        "quality",
        &["low", "medium", "high", "auto"],
    );
    copy_non_empty_string(input, &mut output, "size");

    serde_json::to_vec(&Value::Object(output))
        .map_err(|error| format!("Codex 图片生成请求序列化失败: {error}"))
}

/// 把标准 `/images/edits` multipart 请求转换为 Codex edits JSON。
///
/// OpenAI SDK 可能把图片数组编码成重复的 `image`，也可能使用 `image[]`；两种形式都
/// 接受。mask 等 Codex 请求结构没有的文件字段会被完整消费但不会进入上游请求。
pub async fn transform_edits_body(content_type: &str, body: Vec<u8>) -> Result<Vec<u8>, String> {
    let boundary = multer::parse_boundary(content_type)
        .map_err(|error| format!("图片编辑请求缺少合法 multipart boundary: {error}"))?;
    let body_stream = stream::once(async move { Result::<Vec<u8>, Infallible>::Ok(body) });
    let mut multipart = multer::Multipart::new(body_stream, boundary);
    let mut prompt = None;
    let mut images = Vec::new();
    let mut text_fields = Map::new();

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|error| format!("解析图片编辑 multipart 字段失败: {error}"))?
    {
        let name = field.name().unwrap_or_default().to_owned();
        let file_name = field.file_name().map(str::to_owned);
        let content_type = field.content_type().map(ToString::to_string);
        let bytes = field
            .bytes()
            .await
            .map_err(|error| format!("读取图片编辑 multipart 字段 `{name}` 失败: {error}"))?;

        if matches!(name.as_str(), "image" | "image[]") {
            if bytes.is_empty() {
                return Err("图片编辑请求中的 image 文件不能为空".to_owned());
            }
            let media_type =
                resolve_image_media_type(content_type.as_deref(), file_name.as_deref());
            let encoded = BASE64_STANDARD.encode(&bytes);
            images.push(Value::Object(Map::from_iter([(
                "image_url".to_owned(),
                Value::String(format!("data:{media_type};base64,{encoded}")),
            )])));
            continue;
        }

        // 文件型 mask 是标准 Images API 参数，但 Codex edits JSON 没有对应字段。
        // 其他未知文件字段同样只消费，不尝试把二进制误解为文本参数。
        if file_name.is_some() || name == "mask" {
            continue;
        }
        let text = String::from_utf8(bytes.to_vec())
            .map_err(|error| format!("图片编辑字段 `{name}` 不是 UTF-8 文本: {error}"))?;
        if name == "prompt" {
            prompt = Some(text);
        } else if !name.is_empty() {
            text_fields.insert(name, Value::String(text));
        }
    }

    let prompt = prompt
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "图片编辑请求缺少非空 prompt".to_owned())?;
    if images.is_empty() {
        return Err("图片编辑请求至少需要一个 image 文件".to_owned());
    }

    let mut output = Map::new();
    output.insert("images".to_owned(), Value::Array(images));
    output.insert("prompt".to_owned(), Value::String(prompt.to_owned()));
    output.insert(
        "model".to_owned(),
        Value::String(CODEX_IMAGE_MODEL.to_owned()),
    );
    copy_enum(
        &text_fields,
        &mut output,
        "background",
        &["transparent", "opaque", "auto"],
    );
    copy_positive_integer_from_text(&text_fields, &mut output, "n");
    copy_enum(
        &text_fields,
        &mut output,
        "quality",
        &["low", "medium", "high", "auto"],
    );
    copy_non_empty_string(&text_fields, &mut output, "size");

    serde_json::to_vec(&Value::Object(output))
        .map_err(|error| format!("Codex 图片编辑请求序列化失败: {error}"))
}

/// 将 Codex 图片响应统一压缩成 OpenAI Images API 的 base64 data 数组。
///
/// Codex 独立 Images 端点通常直接返回 `data[].b64_json`；工具执行链则会把图片放进
/// `input_image.image_url`。这里同时识别两种形态，使函数既可用于直连端点，也可承接
/// 未来宿主传入的工具输出 envelope。
pub fn transform_image_response_body(body: &[u8]) -> Result<Vec<u8>, String> {
    let value: Value = serde_json::from_slice(body)
        .map_err(|error| format!("Codex 图片响应不是合法 JSON: {error}"))?;
    let mut images = direct_b64_images(&value);
    if images.is_empty() {
        collect_input_image_base64(&value, &mut images);
    }
    if images.is_empty() {
        return Err("Codex 图片响应中没有 data[].b64_json 或 input_image base64".to_owned());
    }

    let data = images
        .into_iter()
        .map(|b64_json| {
            Value::Object(Map::from_iter([(
                "b64_json".to_owned(),
                Value::String(b64_json),
            )]))
        })
        .collect();
    serde_json::to_vec(&Value::Object(Map::from_iter([(
        "data".to_owned(),
        Value::Array(data),
    )])))
    .map_err(|error| format!("OpenAI 图片响应序列化失败: {error}"))
}

fn required_non_empty_string<'a>(
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

fn copy_enum(
    input: &Map<String, Value>,
    output: &mut Map<String, Value>,
    field: &str,
    allowed: &[&str],
) {
    let Some(value) = input
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| allowed.contains(value))
    else {
        return;
    };
    output.insert(field.to_owned(), Value::String(value.to_owned()));
}

fn copy_non_empty_string(input: &Map<String, Value>, output: &mut Map<String, Value>, field: &str) {
    let Some(value) = input
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return;
    };
    output.insert(field.to_owned(), Value::String(value.to_owned()));
}

fn copy_positive_integer(input: &Map<String, Value>, output: &mut Map<String, Value>, field: &str) {
    let Some(value) = input
        .get(field)
        .and_then(Value::as_u64)
        .filter(|value| *value > 0)
    else {
        return;
    };
    output.insert(field.to_owned(), Value::from(value));
}

fn copy_positive_integer_from_text(
    input: &Map<String, Value>,
    output: &mut Map<String, Value>,
    field: &str,
) {
    let Some(value) = input
        .get(field)
        .and_then(Value::as_str)
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
    else {
        return;
    };
    output.insert(field.to_owned(), Value::from(value));
}

fn resolve_image_media_type(content_type: Option<&str>, file_name: Option<&str>) -> String {
    if let Some(content_type) = content_type
        .map(str::trim)
        .filter(|value| value.starts_with("image/"))
    {
        return content_type.to_owned();
    }
    file_name
        .and_then(|name| mime_guess::from_path(name).first_raw())
        .filter(|value| value.starts_with("image/"))
        .unwrap_or("application/octet-stream")
        .to_owned()
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
