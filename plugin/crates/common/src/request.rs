use serde_json::{Map, Value, json};

use crate::response_context::encode_response_context;

const CODEX_INSTRUCTIONS: &str = include_str!("instructions/codex.txt");
const GPT_5_1_INSTRUCTIONS: &str = include_str!("instructions/gpt5_1.txt");
const GPT_5_2_INSTRUCTIONS: &str = include_str!("instructions/gpt5_2.txt");
const GPT_5_5_INSTRUCTIONS: &str = include_str!("instructions/gpt5_5.txt");
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
    /// 仅非流式下游需要。它保存最终上游请求中会被标准 Response 回显的配置字段，供
    /// buffered 插件在 Codex 终止事件采用精简结构时补齐响应外壳。
    pub response_context: Option<Vec<u8>>,
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
    normalize_prompt(object)?;
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

    // 顶层 instructions 只承载调用方显式指令或 Codex base prompt；system 输入消息单独
    // 转成 developer，不能再复制到 instructions，否则同一条高优先级指令会进入上下文两次。
    ensure_instructions(object, &original_model);
    normalize_system_messages(object);
    normalize_input(object);

    let response_context = if downstream_streaming {
        None
    } else {
        Some(encode_response_context(&value)?)
    };
    let body =
        serde_json::to_vec(&value).map_err(|error| format!("改造后的请求体无法序列化: {error}"))?;
    Ok(OAuthTransformOutput {
        body,
        downstream_streaming,
        response_context,
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

/// 标准 Responses API 中非空的 `prompt` 用于引用可复用 Prompt 模板，并不是普通文本
/// 提示词。ChatGPT Codex OAuth 上游会明确拒绝非空值，因此必须在转发前返回错误，不能
/// 静默删除后继续请求；`null` 仅表示未设置，直接删除以免向上游发送无意义字段。
fn normalize_prompt(object: &mut Map<String, Value>) -> Result<(), String> {
    match object.get("prompt") {
        None => Ok(()),
        Some(Value::Null) => {
            object.remove("prompt");
            Ok(())
        }
        Some(_) => Err(
            "ChatGPT Codex OAuth endpoint 不支持 prompt，请改用 instructions 和 input".to_owned(),
        ),
    }
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

fn normalize_input(object: &mut Map<String, Value>) {
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
}

/// ChatGPT Codex 输入历史统一使用 developer 表达高优先级消息。这里仅改 role 并保留消息
/// 在原始位置，不把文本提升到顶层 instructions，避免改变顺序或重复注入同一内容。
fn normalize_system_messages(object: &mut Map<String, Value>) {
    let Some(Value::Array(input)) = object.get_mut("input") else {
        return;
    };
    for item in input {
        let Some(item) = item.as_object_mut() else {
            continue;
        };
        if item.get("role").and_then(Value::as_str) != Some("system") {
            continue;
        }
        item.insert("role".to_owned(), Value::String("developer".to_owned()));
    }
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
        assert_eq!(value["tools"][0]["name"], "lookup");
        assert!(value["tools"][0].get("function").is_none());
        assert_eq!(value["tool_choice"]["name"], "lookup");
        assert_eq!(value["input"][0]["role"], "developer");
        assert_eq!(value["input"][1]["id"], "rs_1");
        assert!(value["input"][1].get("summary").is_none());
        let instructions = value["instructions"].as_str().unwrap();
        assert!(instructions.starts_with("You are Codex"));
        assert!(!instructions.contains("system rule"));
        assert!(instructions.contains("# Personality"));
    }

    #[test]
    fn input_string_is_normalized_while_image_fields_are_preserved() {
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
        assert_eq!(value["input"][0]["content"].as_array().unwrap().len(), 2);
        assert_eq!(
            value["input"][0]["content"][0]["image_url"],
            "data:image/png;base64,"
        );
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
    fn input_item_ids_and_call_ids_are_preserved() {
        let long_call_id = format!("call_{}", "x".repeat(80));
        let body = json!({
            "model": "gpt-5.5",
            "tools": [{"type":"function", "name":"lookup"}],
            "input": [
                {"type":"item_reference", "id":"call_ref"},
                {"type":"message", "role":"user", "id":"item_message", "content":"hello"},
                {"type":"function_call", "id":"item_call", "call_id":long_call_id.clone(), "name":"lookup", "arguments":"{}"},
                {"type":"reasoning", "id":"rs_1", "encrypted_content":"secret"}
            ]
        });
        let output = transform_oauth_body(&serde_json::to_vec(&body).unwrap()).unwrap();
        let value: Value = serde_json::from_slice(&output.body).unwrap();
        assert_eq!(value["input"][0]["id"], "call_ref");
        assert_eq!(value["input"][1]["id"], "item_message");
        assert_eq!(value["input"][2]["id"], "item_call");
        assert_eq!(value["input"][2]["call_id"], long_call_id);
        assert_eq!(value["input"][3]["id"], "rs_1");
        assert!(value["input"][3].get("summary").is_none());
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
        assert_eq!(value["text"]["verbosity"], "high");
        assert_eq!(value["text"]["format"]["type"], "text");
        assert!(
            value["instructions"]
                .as_str()
                .unwrap()
                .starts_with("You are GPT-5.2 running in the Codex CLI")
        );
    }
}
