//! Codex OAuth 请求转换插件的独立业务实现。

mod request;

#[cfg(target_arch = "wasm32")]
mod component;

use request::transform_oauth_body;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginError {
    pub code: String,
    pub message: String,
}

impl PluginError {
    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for PluginError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for PluginError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub name: String,
    pub value: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountResource {
    pub access_token: String,
    pub chatgpt_account_id: Option<String>,
    pub chatgpt_account_is_fedramp: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestTransformInput {
    pub account: AccountResource,
    pub headers: Vec<Header>,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestTransformOutput {
    pub headers: Vec<Header>,
    pub body: Vec<u8>,
    pub plugin_context: Vec<u8>,
}

/// 执行完整的 Codex OAuth 请求插件逻辑。该入口不依赖 WASM/WIT，可被协议验证服务直接调用。
pub fn transform_request(
    input: RequestTransformInput,
) -> Result<RequestTransformOutput, PluginError> {
    let transformed = transform_oauth_body(&input.body)
        .map_err(|message| PluginError::new("invalid_oauth_responses_body", message))?;
    let headers = build_account_headers(
        input.headers,
        &input.account.access_token,
        input.account.chatgpt_account_id.as_deref(),
        input.account.chatgpt_account_is_fedramp,
    );
    Ok(RequestTransformOutput {
        headers,
        body: transformed.body,
        plugin_context: serde_json::to_vec(&serde_json::json!({
            "stream": transformed.downstream_streaming,
        }))
        .map_err(|_| {
            PluginError::new(
                "serialize_plugin_context_failed",
                "无法序列化 plugin-context JSON",
            )
        })?,
    })
}

const FALLBACK_CODEX_CLIENT: &str = "codex_cli_rs";
const FALLBACK_CODEX_USER_AGENT: &str =
    "codex_cli_rs/0.144.1 (Ubuntu 22.4.0; x86_64) xterm-256color";

fn build_account_headers(
    headers: Vec<Header>,
    access_token: &str,
    account_id: Option<&str>,
    fedramp: bool,
) -> Vec<Header> {
    let (user_agent, originator) = resolve_codex_identity(&headers);
    let mut output = headers
        .into_iter()
        .filter(|header| should_forward_account_header(&header.name))
        .collect::<Vec<_>>();

    append_unique_text(&mut output, "accept", "text/event-stream");
    append_text_if_missing(&mut output, "content-type", "application/json");
    append_unique_text(&mut output, "openai-beta", "responses=experimental");
    append_unique(&mut output, "user-agent", user_agent);
    append_unique_text(&mut output, "originator", &originator);
    append_unique_text(
        &mut output,
        "authorization",
        format!("Bearer {}", access_token.trim()),
    );
    if let Some(account_id) = account_id.map(str::trim).filter(|value| !value.is_empty()) {
        append_unique_text(&mut output, "chatgpt-account-id", account_id);
    }
    if fedramp {
        append_unique_text(&mut output, "x-openai-fedramp", "true");
    }
    output
}

fn should_forward_account_header(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    matches!(
        name.as_str(),
        "accept-language"
            | "content-type"
            | "user-agent"
            | "originator"
            | "x-codex-beta-features"
            | "x-codex-turn-state"
            | "x-codex-turn-metadata"
            | "x-openai-internal-codex-responses-lite"
    ) && !is_hop_by_hop(&name)
}

fn resolve_codex_identity(headers: &[Header]) -> (Vec<u8>, String) {
    let downstream = headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case("user-agent"));
    let Some(user_agent) = downstream
        .and_then(|header| std::str::from_utf8(&header.value).ok())
        .map(str::trim)
    else {
        return fallback_codex_identity();
    };
    let Some((leading, suffix)) = user_agent.split_once('/') else {
        return fallback_codex_identity();
    };

    if let Some(originator) = supported_codex_client(leading.trim()) {
        return (format!("{originator}/{suffix}").into_bytes(), originator);
    }
    if let Some(originator) = codex_user_agent_trailer_name(user_agent)
        .filter(|name| !name.contains('/'))
        .and_then(supported_codex_client)
    {
        return (format!("{originator}/{suffix}").into_bytes(), originator);
    }
    fallback_codex_identity()
}

fn fallback_codex_identity() -> (Vec<u8>, String) {
    (
        FALLBACK_CODEX_USER_AGENT.as_bytes().to_vec(),
        FALLBACK_CODEX_CLIENT.to_owned(),
    )
}

fn supported_codex_client(client: &str) -> Option<String> {
    let client = client.trim();
    if client.is_empty()
        || client.len() > 64
        || !client.bytes().all(|byte| (0x20..=0x7e).contains(&byte))
    {
        return None;
    }
    let lower = client.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "codex_cli_rs"
            | "codex-tui"
            | "codex_vscode"
            | "codex_vscode_copilot"
            | "codex_app"
            | "codex_chatgpt_desktop"
            | "codex_atlas"
            | "codex_exec"
            | "codex_sdk_ts"
    ) {
        return Some(lower);
    }
    lower.starts_with("codex ").then(|| client.to_owned())
}

fn codex_user_agent_trailer_name(user_agent: &str) -> Option<&str> {
    let trailer = user_agent.rsplit_once('(')?.1.split_once(')')?.0.trim();
    let name = trailer
        .split_once(';')
        .map_or(trailer, |(name, _)| name)
        .trim();
    (!name.is_empty()).then_some(name)
}

fn is_hop_by_hop(name: &str) -> bool {
    matches!(
        name,
        "connection"
            | "transfer-encoding"
            | "te"
            | "trailer"
            | "upgrade"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "proxy-connection"
    )
}

fn append_unique_text(headers: &mut Vec<Header>, name: &str, value: impl AsRef<str>) {
    append_unique(headers, name, value.as_ref().as_bytes().to_vec());
}

fn append_text_if_missing(headers: &mut Vec<Header>, name: &str, value: impl AsRef<str>) {
    if headers
        .iter()
        .any(|header| header.name.eq_ignore_ascii_case(name) && !header.value.is_empty())
    {
        return;
    }
    append_unique_text(headers, name, value);
}

fn append_unique(headers: &mut Vec<Header>, name: &str, value: Vec<u8>) {
    headers.retain(|header| !header.name.eq_ignore_ascii_case(name));
    headers.push(Header {
        name: name.to_owned(),
        value,
    });
}
