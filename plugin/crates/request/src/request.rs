use serde_json::{Map, Value, json};

/// 调用方没有提供 instructions 时使用固定的基础提示词。model 是不透明的上游路由标识，
/// 不能再参与提示词选择，否则新增模型或自定义模型 ID 会被插件隐式赋予额外语义。
const DEFAULT_CODEX_INSTRUCTIONS: &str = include_str!("instructions/gpt5_5.txt");
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
    /// WIT ABI 预留字段。当前插件不再通过请求上下文补齐响应字段，始终返回 `None`。
    /// 保留字段仅为避免改变宿主与已发布组件之间的接口形状。
    pub response_context: Option<Vec<u8>>,
}

pub fn transform_oauth_body(body: &[u8]) -> Result<OAuthTransformOutput, String> {
    let mut value = serde_json::from_slice::<Value>(body)
        .map_err(|error| format!("请求体不是合法 JSON: {error}"))?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| "GPT Responses 请求体必须是 JSON object".to_owned())?;
    let downstream_streaming = object.get("stream").and_then(Value::as_bool) == Some(true);

    // model 仅校验结构，不做 trim、alias 归一化、后缀解析或任何信息推导；JSON 中的原始值
    // 将被逐字保留并发送给上游。
    require_model(object)?;
    reject_previous_response_id(object)?;
    normalize_prompt(object)?;

    // ChatGPT internal Responses 固定使用不落库的流式协议。即使下游显式给出相反值，
    // OAuth 插件也拥有最终上游请求语义，因此这里直接覆盖。
    object.insert("store".to_owned(), Value::Bool(false));
    object.insert("stream".to_owned(), Value::Bool(true));
    for field in UNSUPPORTED_OAUTH_FIELDS {
        object.remove(*field);
    }

    normalize_reasoning(object);
    normalize_service_tier(object);

    // 顶层 instructions 只承载调用方显式指令或 Codex base prompt；system 输入消息单独
    // 转成 developer，不能再复制到 instructions，否则同一条高优先级指令会进入上下文两次。
    ensure_instructions(object);
    normalize_system_messages(object);
    normalize_input(object);

    let body =
        serde_json::to_vec(&value).map_err(|error| format!("改造后的请求体无法序列化: {error}"))?;
    Ok(OAuthTransformOutput {
        body,
        downstream_streaming,
        // 非流式响应只能采用上游实际返回的数据；禁止把请求字段作为响应兜底值。
        response_context: None,
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

fn normalize_reasoning(object: &mut Map<String, Value>) {
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

fn ensure_instructions(object: &mut Map<String, Value>) {
    let missing = object
        .get("instructions")
        .and_then(Value::as_str)
        .is_none_or(|instructions| instructions.trim().is_empty());
    if missing {
        object.insert(
            "instructions".to_owned(),
            Value::String(DEFAULT_CODEX_INSTRUCTIONS.to_owned()),
        );
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
        assert_eq!(value["model"], "gpt-5.4-high");
        assert_eq!(value["stream"], true);
        assert!(!output.downstream_streaming);
        assert_eq!(value["store"], false);
        assert!(value.get("temperature").is_none());
        assert!(value.get("user").is_none());
        assert!(value.get("reasoning").is_none());
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
        assert_eq!(value["model"], "gpt-5.3");
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
    fn model_is_preserved_without_deriving_reasoning() {
        let output =
            transform_oauth_body(br#"{"model":"openai/gpt-5.6-sol-xhigh","input":"hello"}"#)
                .unwrap();
        let value: Value = serde_json::from_slice(&output.body).unwrap();
        assert_eq!(value["model"], "openai/gpt-5.6-sol-xhigh");
        assert!(value.get("reasoning").is_none());
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
    fn model_independent_fields_are_normalized() {
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
                .starts_with("You are Codex")
        );
    }
}
