use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

const CODEX_INSTRUCTIONS: &str = include_str!("instructions/codex.txt");
const GPT_5_1_INSTRUCTIONS: &str = include_str!("instructions/gpt5_1.txt");
const GPT_5_2_INSTRUCTIONS: &str = include_str!("instructions/gpt5_2.txt");
const GPT_5_5_INSTRUCTIONS: &str = include_str!("instructions/gpt5_5.txt");
const REASONING_ENCRYPTED_CONTENT: &str = "reasoning.encrypted_content";
const UNSUPPORTED_OAUTH_FIELDS: &[&str] = &[
    "max_output_tokens",
    "temperature",
    "top_p",
    "frequency_penalty",
    "presence_penalty",
    "user",
    "metadata",
    "prompt_cache_retention",
    "safety_identifier",
    "stream_options",
];

/// 将 OAuth 标准 Responses 请求收敛到 ChatGPT Codex internal API 接受的形态。
/// 此函数只处理 JSON 字段，不处理 URL、method、资源调度和重试。
pub struct OAuthTransformOutput {
    pub body: Vec<u8>,
    /// 调用方原始请求是否要求 SSE。OAuth 上游 body 随后仍会固定为 `stream=true`，
    /// 因此这个值必须在覆盖字段前保存，供请求插件选择下游响应交付模式。
    pub downstream_streaming: bool,
}

pub fn transform_oauth_body(body: &[u8]) -> Result<OAuthTransformOutput, String> {
    let mut value = serde_json::from_slice::<Value>(body)
        .map_err(|error| format!("请求体不是合法 JSON: {error}"))?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| "GPT Responses 请求体必须是 JSON object".to_owned())?;
    let downstream_streaming = object.get("stream").and_then(Value::as_bool) == Some(true);

    let original_model = require_model(object)?.to_owned();
    reject_previous_response_id(object)?;
    let preserve_references = has_continuation_input(object);
    let inferred_effort = effort_suffix(&original_model);
    object.insert(
        "model".to_owned(),
        Value::String(normalize_codex_model(&original_model)),
    );

    // ChatGPT internal Responses 固定使用不落库的流式协议。即使下游显式给出相反值，
    // OAuth 插件也拥有最终上游请求语义，因此这里直接覆盖。
    object.insert("store".to_owned(), Value::Bool(false));
    object.insert("stream".to_owned(), Value::Bool(true));
    for field in UNSUPPORTED_OAUTH_FIELDS {
        object.remove(*field);
    }

    normalize_reasoning(object, inferred_effort);
    normalize_service_tier(object);
    strip_unsupported_verbosity(object);

    // sub2api 在进入 OAuth transform 前已按原始模型补齐 base instructions。先补默认值、
    // 再提升 system 消息，才能得到“system 文本 + base prompt”的相同顺序。
    ensure_instructions(object, &original_model);
    promote_system_messages(object);
    normalize_input(object, preserve_references);
    sanitize_empty_base64_images(object);

    let body =
        serde_json::to_vec(&value).map_err(|error| format!("改造后的请求体无法序列化: {error}"))?;
    Ok(OAuthTransformOutput {
        body,
        downstream_streaming,
    })
}

fn require_model(object: &Map<String, Value>) -> Result<&str, String> {
    let Some(Value::String(model)) = object.get("model") else {
        return Err("GPT Responses 请求体缺少非空字符串 model".to_owned());
    };
    let model = model.trim();
    if model.is_empty() {
        return Err("model 不能为空".to_owned());
    }
    Ok(model)
}

/// HTTP `/v1/responses` 没有 WebSocket v2 的连接态，不能可靠承接
/// `previous_response_id`。真实入口还会在 provider inspect 阶段返回 400；插件侧保留
/// 同样的拒绝，保证组件被单独测试或复用时也不会静默删除该字段后改变调用语义。
fn reject_previous_response_id(object: &Map<String, Value>) -> Result<(), String> {
    let Some(previous_response_id) = object
        .get("previous_response_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(());
    };
    if looks_like_message_id(previous_response_id) {
        return Err(
            "previous_response_id must be a response.id (resp_*), not a message id".to_owned(),
        );
    }
    Err("previous_response_id is only supported on Responses WebSocket v2".to_owned())
}

fn looks_like_message_id(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    let Some(suffix) = ["msg_", "message_", "item_", "chatcmpl_"]
        .into_iter()
        .find_map(|prefix| value.strip_prefix(prefix))
    else {
        return false;
    };
    (1..=256).contains(&suffix.len())
        && suffix
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn has_continuation_input(object: &Map<String, Value>) -> bool {
    object
        .get("input")
        .and_then(Value::as_array)
        .is_some_and(|input| {
            input.iter().filter_map(Value::as_object).any(|item| {
                item.get("type")
                    .and_then(Value::as_str)
                    .is_some_and(|kind| {
                        is_tool_call_item_type(kind.trim()) || kind.trim() == "item_reference"
                    })
            })
        })
}

/// 对齐 sub2api 当前 Codex alias：只归一化已知模型，未知模型保留调用方值，避免插件
/// 在新增模型发布时擅自降级。
pub fn normalize_codex_model(model: &str) -> String {
    let trimmed = model.trim();
    let model_id = trimmed.rsplit('/').next().unwrap_or(trimmed);
    let key = model_id
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("-")
        .to_ascii_lowercase();

    let exact = match key.as_str() {
        "gpt-5.6-sol" => Some("gpt-5.6-sol"),
        "gpt-5.6-terra" => Some("gpt-5.6-terra"),
        "gpt-5.6-luna" => Some("gpt-5.6-luna"),
        "gpt-5.5" => Some("gpt-5.5"),
        "gpt-5.5-pro" => Some("gpt-5.5-pro"),
        "codex-auto-review" => Some("codex-auto-review"),
        "gpt-5.4" => Some("gpt-5.4"),
        "gpt-5.4-mini" => Some("gpt-5.4-mini"),
        "gpt-5.4-none"
        | "gpt-5.4-low"
        | "gpt-5.4-medium"
        | "gpt-5.4-high"
        | "gpt-5.4-xhigh"
        | "gpt-5.4-chat-latest" => Some("gpt-5.4"),
        "gpt-5.3"
        | "gpt-5.3-none"
        | "gpt-5.3-low"
        | "gpt-5.3-medium"
        | "gpt-5.3-high"
        | "gpt-5.3-xhigh"
        | "gpt-5.3-codex"
        | "gpt-5.3-codex-low"
        | "gpt-5.3-codex-medium"
        | "gpt-5.3-codex-high"
        | "gpt-5.3-codex-xhigh" => Some("gpt-5.3-codex"),
        "gpt-5.2" | "gpt-5.2-none" | "gpt-5.2-low" | "gpt-5.2-medium" | "gpt-5.2-high"
        | "gpt-5.2-xhigh" | "gpt-5.2-codex" => Some("gpt-5.2"),
        "gpt-5" | "gpt-5-mini" | "gpt-5-nano" | "gpt-5.1" => Some("gpt-5.4"),
        "gpt-5.1-codex" | "gpt-5.1-codex-max" | "gpt-5.1-codex-mini" | "codex-mini-latest"
        | "gpt-5-codex" => Some("gpt-5.3-codex"),
        _ => None,
    };
    if let Some(exact) = exact {
        return exact.to_owned();
    }

    for (prefix, target) in [
        ("gpt-5.6-sol", "gpt-5.6-sol"),
        ("gpt-5.6-terra", "gpt-5.6-terra"),
        ("gpt-5.6-luna", "gpt-5.6-luna"),
        ("gpt-5.3-codex", "gpt-5.3-codex"),
        ("gpt-5.4-mini", "gpt-5.4-mini"),
        ("gpt-5.4-nano", "gpt-5.4-nano"),
        ("gpt-5.5-pro", "gpt-5.5-pro"),
        ("gpt-5.5", "gpt-5.5"),
        ("gpt-5.4", "gpt-5.4"),
        ("gpt-5.2", "gpt-5.2"),
    ] {
        if key == prefix
            || key
                .strip_prefix(prefix)
                .and_then(|suffix| suffix.strip_prefix('-'))
                .is_some_and(is_known_model_suffix)
        {
            return target.to_owned();
        }
    }
    trimmed.to_owned()
}

fn is_known_model_suffix(suffix: &str) -> bool {
    matches!(
        suffix,
        "none" | "minimal" | "low" | "medium" | "high" | "xhigh"
    ) || is_date_suffix(suffix)
}

fn is_date_suffix(suffix: &str) -> bool {
    let parts = suffix.split('-').collect::<Vec<_>>();
    parts.len() == 3
        && parts[0].len() == 4
        && parts[1].len() == 2
        && parts[2].len() == 2
        && parts
            .iter()
            .all(|part| part.bytes().all(|byte| byte.is_ascii_digit()))
}

fn effort_suffix(model: &str) -> Option<&'static str> {
    let suffix = model.trim().rsplit('-').next()?;
    match suffix.to_ascii_lowercase().as_str() {
        "minimal" | "none" => Some("none"),
        "low" => Some("low"),
        "medium" => Some("medium"),
        "high" => Some("high"),
        "xhigh" => Some("xhigh"),
        _ => None,
    }
}

fn normalize_reasoning(object: &mut Map<String, Value>, inferred_effort: Option<&str>) {
    if !object.contains_key("reasoning")
        && let Some(effort) = inferred_effort
    {
        object.insert("reasoning".to_owned(), json!({"effort": effort}));
    }
    let Some(Value::Object(reasoning)) = object.get_mut("reasoning") else {
        return;
    };
    if reasoning
        .get("effort")
        .and_then(Value::as_str)
        .is_some_and(|effort| effort.eq_ignore_ascii_case("minimal"))
    {
        reasoning.insert("effort".to_owned(), Value::String("none".to_owned()));
    }
    if reasoning.is_empty() {
        return;
    }

    match object.get_mut("include") {
        None | Some(Value::Null) => {
            object.insert(
                "include".to_owned(),
                Value::Array(vec![Value::String(REASONING_ENCRYPTED_CONTENT.to_owned())]),
            );
        }
        Some(Value::Array(include)) => {
            if !include
                .iter()
                .any(|item| item.as_str() == Some(REASONING_ENCRYPTED_CONTENT))
            {
                include.push(Value::String(REASONING_ENCRYPTED_CONTENT.to_owned()));
            }
        }
        Some(_) => {
            // 异常类型保持原样，避免用“修复”之名覆盖调用方数据。
        }
    }
}

/// 官方 Responses 将 `fast` 定义为 `priority` 的请求别名，响应也统一回显
/// `priority`。这里只转换这一组等价标准值；其他值保持原样，由上游按自身能力校验。
fn normalize_service_tier(object: &mut Map<String, Value>) {
    if object.get("service_tier").and_then(Value::as_str) == Some("fast") {
        object.insert(
            "service_tier".to_owned(),
            Value::String("priority".to_owned()),
        );
    }
}

fn strip_unsupported_verbosity(object: &mut Map<String, Value>) {
    let supports_verbosity = object
        .get("model")
        .and_then(Value::as_str)
        .is_none_or(model_supports_verbosity);
    if supports_verbosity {
        return;
    }
    if let Some(Value::Object(text)) = object.get_mut("text") {
        text.remove("verbosity");
    }
}

fn model_supports_verbosity(model: &str) -> bool {
    let Some(version) = model.trim().strip_prefix("gpt-") else {
        return true;
    };
    let major_digits = version.bytes().take_while(u8::is_ascii_digit).count();
    let Some(major) = version[..major_digits].parse::<u32>().ok() else {
        return false;
    };
    if major > 5 {
        return true;
    }
    if major < 5 {
        return false;
    }
    let Some(minor_text) = version[major_digits..].strip_prefix('.') else {
        return true;
    };
    let minor_digits = minor_text.bytes().take_while(u8::is_ascii_digit).count();
    if minor_digits == 0 {
        return true;
    }
    minor_text[..minor_digits]
        .parse::<u32>()
        .is_ok_and(|minor| minor >= 3)
}

fn normalize_input(object: &mut Map<String, Value>, preserve_references: bool) {
    if matches!(object.get("input"), Some(Value::String(_))) {
        let Some(Value::String(input)) = object.remove("input") else {
            unreachable!("input 已确认是 string");
        };
        object.insert(
            "input".to_owned(),
            if input.trim().is_empty() {
                Value::Array(Vec::new())
            } else {
                Value::Array(vec![json!({
                    "type": "message",
                    "role": "user",
                    "content": input,
                })])
            },
        );
    }
    let Some(Value::Array(input)) = object.get_mut("input") else {
        return;
    };
    let mut normalized = Vec::with_capacity(input.len());
    for mut item in std::mem::take(input) {
        let Some(item_object) = item.as_object_mut() else {
            normalized.push(item);
            continue;
        };
        let item_type = item_object
            .get("type")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default()
            .to_owned();
        if item_type == "reasoning" {
            item_object.remove("id");
            if !item_object.contains_key("summary") || item_object["summary"].is_null() {
                item_object.insert("summary".to_owned(), Value::Array(Vec::new()));
            }
            normalized.push(item);
            continue;
        }

        if item_type == "item_reference" {
            if !preserve_references {
                continue;
            }
            if let Some(Value::String(id)) = item_object.get_mut("id")
                && id.starts_with("call_")
            {
                *id = normalize_call_id(id);
            }
            normalized.push(item);
            continue;
        }

        if is_tool_call_item_type(&item_type) {
            let call_id = item_object
                .get("call_id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned);
            if let Some(call_id) = call_id {
                item_object.insert(
                    "call_id".to_owned(),
                    Value::String(normalize_call_id(&call_id)),
                );
            }
        }

        if !preserve_references {
            item_object.remove("id");
        } else if is_tool_call_input_type(&item_type) {
            let invalid_id = item_object
                .get("id")
                .and_then(Value::as_str)
                .is_some_and(|id| !id.is_empty() && !id.starts_with("fc"));
            if invalid_id {
                item_object.remove("id");
            }
        } else if item_type == "message" {
            let invalid_id = item_object
                .get("id")
                .and_then(Value::as_str)
                .is_some_and(|id| !id.is_empty() && !id.starts_with("msg"));
            if invalid_id {
                item_object.remove("id");
            }
        }

        normalized.push(item);
    }
    *input = normalized;
}

fn normalize_call_id(id: &str) -> String {
    let id = id.trim();
    let normalized = if id.starts_with("fc") {
        id.to_owned()
    } else if let Some(suffix) = id.strip_prefix("call_") {
        format!("fc_{suffix}")
    } else {
        format!("fc_{id}")
    };
    if normalized.len() <= 64 {
        normalized
    } else {
        // 使用同一固定 domain separator，让不同实例、不同语言实现对同一超长 call id
        // 得到完全相同且满足上游 64-byte 限制的值。
        let digest = Sha256::digest(format!("sub2api:codex-call-id:v1:{normalized}").as_bytes());
        format!("fc_{}", &hex::encode(digest)[..61])
    }
}

fn is_tool_call_item_type(kind: &str) -> bool {
    matches!(
        kind,
        "function_call"
            | "local_shell_call"
            | "tool_search_call"
            | "custom_tool_call"
            | "function_call_output"
            | "custom_tool_call_output"
            | "tool_search_output"
    )
}

fn is_tool_call_input_type(kind: &str) -> bool {
    matches!(
        kind,
        "function_call" | "local_shell_call" | "tool_search_call" | "custom_tool_call"
    )
}

fn promote_system_messages(object: &mut Map<String, Value>) {
    let Some(Value::Array(input)) = object.get_mut("input") else {
        return;
    };
    let mut texts = Vec::new();
    for item in input {
        let Some(item) = item.as_object_mut() else {
            continue;
        };
        if item.get("role").and_then(Value::as_str) != Some("system") {
            continue;
        }
        if let Some(content) = item.get("content") {
            let text = content_to_text(content);
            if !text.trim().is_empty() {
                texts.push(text);
            }
        }
        item.insert("role".to_owned(), Value::String("developer".to_owned()));
    }
    if texts.is_empty() {
        return;
    }
    let promoted = texts.join("\n\n");
    let existing = object
        .get("instructions")
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty());
    object.insert(
        "instructions".to_owned(),
        Value::String(match existing {
            Some(existing) => format!("{promoted}\n\n{existing}"),
            None => promoted,
        }),
    );
}

fn ensure_instructions(object: &mut Map<String, Value>, model: &str) {
    let missing = object
        .get("instructions")
        .and_then(Value::as_str)
        .is_none_or(|instructions| instructions.trim().is_empty());
    if missing {
        object.insert(
            "instructions".to_owned(),
            Value::String(codex_instructions_for_model(model).to_owned()),
        );
    }
}

/// 与 sub2api 的 `CodexBaseInstructionsForModel` 使用同一组内嵌 prompt 和选择规则。
/// 这里刻意按下游原始 model 选择，避免模型 alias 归一化改变 base prompt。
fn codex_instructions_for_model(model: &str) -> &'static str {
    let model = model.trim().to_ascii_lowercase();
    if model.contains("codex") {
        CODEX_INSTRUCTIONS
    } else if model.starts_with("gpt-5.5") {
        GPT_5_5_INSTRUCTIONS
    } else if model.starts_with("gpt-5.2") {
        GPT_5_2_INSTRUCTIONS
    } else if model.starts_with("gpt-5.1") {
        GPT_5_1_INSTRUCTIONS
    } else {
        GPT_5_5_INSTRUCTIONS
    }
}

fn content_to_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(Value::as_object)
            .filter_map(|part| {
                let kind = part.get("type").and_then(Value::as_str)?;
                (kind == "input_text")
                    .then(|| part.get("text").and_then(Value::as_str).map(str::to_owned))
                    .flatten()
            })
            .collect::<Vec<_>>()
            .join(""),
        Value::Null => String::new(),
        _ => String::new(),
    }
}

fn sanitize_empty_base64_images(object: &mut Map<String, Value>) {
    let Some(Value::Array(input)) = object.get_mut("input") else {
        return;
    };
    input.retain_mut(|item| {
        let Some(item_object) = item.as_object_mut() else {
            return true;
        };
        if is_empty_base64_image(item_object) {
            return false;
        }
        if let Some(Value::Array(content)) = item_object.get_mut("content") {
            content.retain(|part| !part.as_object().is_some_and(is_empty_base64_image));
            if content.is_empty() {
                return false;
            }
        }
        true
    });
}

fn is_empty_base64_image(part: &Map<String, Value>) -> bool {
    if part.get("type").and_then(Value::as_str) != Some("input_image") {
        return false;
    }
    let Some(url) = part.get("image_url").and_then(Value::as_str) else {
        return false;
    };
    let Some((metadata, payload)) = url
        .strip_prefix("data:")
        .and_then(|url| url.split_once(','))
    else {
        return false;
    };
    metadata
        .split(';')
        .any(|token| token.trim().eq_ignore_ascii_case("base64"))
        && payload.trim().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oauth_transform_remembers_downstream_stream_before_forcing_upstream_sse() {
        let output =
            transform_oauth_body(br#"{"model":"gpt-5.4","stream":true,"input":"hello"}"#).unwrap();
        assert!(output.downstream_streaming);
        let upstream: Value = serde_json::from_slice(&output.body).unwrap();
        assert_eq!(upstream["stream"], true);
    }

    #[test]
    fn oauth_body_applies_codex_request_transforms() {
        let body = br#"{
            "model":"gpt-5.4-high",
            "stream":false,
            "store":true,
            "temperature":0.2,
            "user":"u",
            "input":[
                {"type":"message","role":"system","content":"system rule"},
                {"type":"reasoning","id":"rs_1","encrypted_content":"secret"}
            ],
            "tools":[{"type":"function","name":"lookup","parameters":{"type":"object"}}],
            "tool_choice":{"type":"function","name":"lookup"}
        }"#;
        let output = transform_oauth_body(body).unwrap();
        let value: Value = serde_json::from_slice(&output.body).unwrap();
        assert_eq!(value["model"], "gpt-5.4");
        assert_eq!(value["stream"], true);
        assert!(!output.downstream_streaming);
        assert_eq!(value["store"], false);
        assert!(value.get("temperature").is_none());
        assert!(value.get("user").is_none());
        assert_eq!(value["reasoning"]["effort"], "high");
        assert_eq!(value["include"][0], REASONING_ENCRYPTED_CONTENT);
        assert_eq!(value["tools"][0]["name"], "lookup");
        assert!(value["tools"][0].get("function").is_none());
        assert_eq!(value["tool_choice"]["name"], "lookup");
        assert_eq!(value["input"][0]["role"], "developer");
        assert_eq!(value["input"][1]["summary"], json!([]));
        assert!(value["input"][1].get("id").is_none());
        let instructions = value["instructions"].as_str().unwrap();
        assert!(instructions.starts_with("system rule\n\nYou are Codex"));
        assert!(instructions.contains("# Personality"));
    }

    #[test]
    fn input_string_and_empty_image_are_normalized() {
        let output = transform_oauth_body(
            br#"{"model":"gpt-5.3","input":"hello","tools":[{"type":"image_generation","output_format":"png","output_compression":80}]}"#,
        )
        .unwrap();
        let value: Value = serde_json::from_slice(&output.body).unwrap();
        assert_eq!(value["model"], "gpt-5.3-codex");
        assert_eq!(value["input"][0]["role"], "user");
        assert_eq!(value["tools"][0]["output_format"], "png");
        assert_eq!(value["tools"][0]["output_compression"], 80);
        assert!(
            value["instructions"]
                .as_str()
                .unwrap()
                .starts_with("You are Codex")
        );

        let output = transform_oauth_body(
            br#"{"model":"gpt-5.4","input":[{"type":"message","role":"user","content":[{"type":"input_image","image_url":"data:image/png;base64,"},{"type":"input_text","text":"keep"}]}]}"#,
        )
        .unwrap();
        let value: Value = serde_json::from_slice(&output.body).unwrap();
        assert_eq!(value["input"][0]["content"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn namespace_tools_and_qualified_calls_are_preserved() {
        let output = transform_oauth_body(
            br#"{
                "model":"gpt-5.4",
                "tools":[
                    {"type":"function","name":"plain"},
                    {"type":"namespace","name":"image_gen","tools":[{"type":"function","name":"imagegen"}]},
                    {"type":"namespace","name":"collaboration","tools":[{"type":"function","name":"spawn_agent","parameters":{"type":"object"}}]}
                ],
                "input":[{"type":"function_call","name":"spawn_agent","namespace":"collaboration","arguments":"{}"}]
            }"#,
        )
        .unwrap();
        let value: Value = serde_json::from_slice(&output.body).unwrap();
        assert_eq!(value["tools"][0]["name"], "plain");
        assert_eq!(value["tools"][1]["type"], "namespace");
        assert_eq!(value["tools"][1]["name"], "image_gen");
        assert_eq!(value["tools"][2]["type"], "namespace");
        assert_eq!(value["tools"][2]["name"], "collaboration");
        assert_eq!(value["tools"][2]["tools"][0]["name"], "spawn_agent");
        assert_eq!(value["input"][0]["name"], "spawn_agent");
        assert_eq!(value["input"][0]["namespace"], "collaboration");
    }

    #[test]
    fn unknown_model_is_not_silently_replaced() {
        assert_eq!(
            normalize_codex_model("vendor/future-model"),
            "vendor/future-model"
        );
        assert_eq!(
            normalize_codex_model("openai/gpt-5.6-sol-xhigh"),
            "gpt-5.6-sol"
        );
        assert_eq!(normalize_codex_model("gpt-5.5-2026-07-01"), "gpt-5.5");
    }

    #[test]
    fn oauth_body_rejects_http_previous_response_id_without_changing_prompt_cache_key() {
        let error = transform_oauth_body(
            br#"{"model":"gpt-5.5","previous_response_id":"resp_123","prompt_cache_key":" cache "}"#,
        )
        .err()
        .unwrap();
        assert_eq!(
            error,
            "previous_response_id is only supported on Responses WebSocket v2"
        );

        let output = transform_oauth_body(
            br#"{"model":"gpt-5.5","prompt_cache_key":" cache ","input":"hello"}"#,
        )
        .unwrap();
        let value: Value = serde_json::from_slice(&output.body).unwrap();
        assert_eq!(value["prompt_cache_key"], " cache ");
    }

    #[test]
    fn input_filter_matches_codex_reference_and_call_id_rules() {
        let long_call_id = format!("call_{}", "x".repeat(80));
        let body = json!({
            "model": "gpt-5.5",
            "tools": [{"type":"function", "name":"lookup"}],
            "input": [
                {"type":"item_reference", "id":"call_ref"},
                {"type":"message", "role":"user", "id":"item_message", "content":"hello"},
                {"type":"function_call", "id":"item_call", "call_id":long_call_id, "name":"lookup", "arguments":"{}"},
                {"type":"reasoning", "id":"rs_1", "encrypted_content":"secret"}
            ]
        });
        let output = transform_oauth_body(&serde_json::to_vec(&body).unwrap()).unwrap();
        let value: Value = serde_json::from_slice(&output.body).unwrap();
        assert_eq!(value["input"][0]["id"], "fc_ref");
        assert!(value["input"][1].get("id").is_none());
        assert!(value["input"][2].get("id").is_none());
        let compacted = value["input"][2]["call_id"].as_str().unwrap();
        assert_eq!(
            compacted,
            "fc_c799a41af85be7b0a2ca4f40b252062d5d2c6710dbdda85985547caa83ceb"
        );
        assert!(value["input"][3].get("id").is_none());
        assert_eq!(value["input"][3]["summary"], json!([]));
    }

    #[test]
    fn model_specific_fields_are_normalized() {
        let output = transform_oauth_body(
            br#"{
                "model":"gpt-5.2",
                "service_tier":"fast",
                "text":{"verbosity":"high","format":{"type":"text"}},
                "input":"hello"
            }"#,
        )
        .unwrap();
        let value: Value = serde_json::from_slice(&output.body).unwrap();
        assert_eq!(value["service_tier"], "priority");
        assert!(value["text"].get("verbosity").is_none());
        assert_eq!(value["text"]["format"]["type"], "text");
        assert!(
            value["instructions"]
                .as_str()
                .unwrap()
                .starts_with("You are GPT-5.2 running in the Codex CLI")
        );
    }
}
