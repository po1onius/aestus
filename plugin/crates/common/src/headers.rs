use std::collections::BTreeSet;

use crate::Header;

const FALLBACK_CODEX_CLIENT: &str = "codex_cli_rs";
const FALLBACK_CODEX_USER_AGENT: &str =
    "codex_cli_rs/0.144.1 (Ubuntu 22.4.0; x86_64) xterm-256color";

/// 响应组件完成安全过滤后对 Content-Type 的处理方式。SSE→JSON 转换必须显式覆盖
/// 上游的 `text/event-stream`，普通 buffered 透传则保留上游原值。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseHeaderMode {
    Preserve,
    Json,
    EventStream,
}

/// OAuth account 请求使用 Codex 白名单重建 header。这样下游的网关凭证、Host、
/// framing header 和任意伪造的 ChatGPT account header 都不会进入上游请求。
pub fn build_account_headers(
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

/// Codex Images 端点使用普通 JSON 响应，不需要 Responses SSE 的 accept/openai-beta。
/// 这里仍复用同一套官方 Codex 客户端身份识别和账号鉴权规则，避免下游伪造账号 header。
pub fn build_image_account_headers(
    headers: Vec<Header>,
    access_token: &str,
    account_id: Option<&str>,
    fedramp: bool,
) -> Vec<Header> {
    let (user_agent, originator) = resolve_codex_identity(&headers);
    let mut output = headers
        .into_iter()
        .filter(|header| matches!(header.name.to_ascii_lowercase().as_str(), "accept-language"))
        .collect::<Vec<_>>();

    append_unique_text(&mut output, "accept", "application/json");
    append_unique_text(&mut output, "content-type", "application/json");
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

/// 响应插件完全接管下游 header，因此在插件侧先删除 hop-by-hop、上游 cookie、上游
/// 鉴权和失效的 content-length。宿主仍会做最终安全校验，这里是业务组件自身的明确契约。
pub fn sanitize_response_headers(headers: Vec<Header>, mode: ResponseHeaderMode) -> Vec<Header> {
    let connection_scoped = connection_scoped_header_names(&headers);
    let mut output = headers
        .into_iter()
        .filter(|header| {
            let name = header.name.to_ascii_lowercase();
            !is_hop_by_hop(&name)
                && !connection_scoped.contains(&name)
                && !matches!(
                    name.as_str(),
                    "host"
                        | "content-length"
                        | "set-cookie"
                        | "authorization"
                        | "x-api-key"
                        | "proxy-authenticate"
                        | "proxy-authorization"
                )
        })
        .collect::<Vec<_>>();
    match mode {
        ResponseHeaderMode::Preserve => {}
        ResponseHeaderMode::Json => {
            append_unique_text(
                &mut output,
                "content-type",
                "application/json; charset=utf-8",
            );
        }
        ResponseHeaderMode::EventStream => {
            append_unique_text(
                &mut output,
                "content-type",
                "text/event-stream; charset=utf-8",
            );
            append_unique_text(&mut output, "cache-control", "no-cache");
            // 与 sub2api 的 HTTP SSE 响应头保持一致。宿主会先清理所有插件提供的
            // hop-by-hop header，再为 stream head 重新写入同一个权威值。
            append_unique_text(&mut output, "connection", "keep-alive");
        }
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

    // CODEX_INTERNAL_ORIGINATOR_OVERRIDE 只会改 UA 首段，真实 clientInfo.name 仍在最后
    // 一个 `(name; version)` trailer 中。sub2api 会用它恢复官方身份并重写首段。
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

fn connection_scoped_header_names(headers: &[Header]) -> BTreeSet<String> {
    headers
        .iter()
        .filter(|header| header.name.eq_ignore_ascii_case("connection"))
        .filter_map(|header| std::str::from_utf8(&header.value).ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn header(name: &str, value: &str) -> Header {
        Header {
            name: name.to_owned(),
            value: value.as_bytes().to_vec(),
        }
    }

    #[test]
    fn oauth_headers_are_rebuilt_from_allowlist() {
        let headers = build_account_headers(
            vec![
                header("authorization", "Bearer downstream"),
                header("connection", "x-remove"),
                header("x-remove", "1"),
                header("session_id", "session"),
                header("conversation_id", "conversation"),
                header("x-codex-installation-id", "device-must-not-forward"),
                header("traceparent", "trace-must-not-forward"),
                header("x-codex-turn-state", "turn"),
                header("content-type", "application/json; charset=utf-8"),
                header("user-agent", "codex_vscode/1.2.3"),
                header("openai-beta", "downstream-value"),
            ],
            "oauth-token",
            Some("account-id"),
            true,
        );
        let value = |name: &str| {
            headers
                .iter()
                .find(|header| header.name == name)
                .map(|header| String::from_utf8_lossy(&header.value).into_owned())
        };
        assert_eq!(
            value("authorization").as_deref(),
            Some("Bearer oauth-token")
        );
        assert_eq!(value("originator").as_deref(), Some("codex_vscode"));
        assert_eq!(
            value("openai-beta").as_deref(),
            Some("responses=experimental")
        );
        assert_eq!(value("chatgpt-account-id").as_deref(), Some("account-id"));
        assert!(headers.iter().all(|header| header.name != "x-remove"));
        assert!(headers.iter().all(|header| header.name != "session_id"));
        assert!(
            headers
                .iter()
                .all(|header| header.name != "conversation_id")
        );
        assert!(
            headers
                .iter()
                .all(|header| header.name != "x-codex-installation-id")
        );
        assert!(headers.iter().all(|header| header.name != "version"));
        assert_eq!(value("x-codex-turn-state").as_deref(), Some("turn"));
        assert_eq!(
            value("content-type").as_deref(),
            Some("application/json; charset=utf-8")
        );
    }

    #[test]
    fn oauth_identity_recovers_official_client_from_user_agent_trailer() {
        let headers = build_account_headers(
            vec![header(
                "user-agent",
                "override/0.144.1 (Ubuntu; x86_64) (codex-tui; 0.144.1)",
            )],
            "oauth-token",
            None,
            false,
        );
        let value = |name: &str| {
            headers
                .iter()
                .find(|header| header.name == name)
                .map(|header| String::from_utf8_lossy(&header.value).into_owned())
        };
        assert_eq!(value("originator").as_deref(), Some("codex-tui"));
        assert_eq!(
            value("user-agent").as_deref(),
            Some("codex-tui/0.144.1 (Ubuntu; x86_64) (codex-tui; 0.144.1)")
        );
    }

    #[test]
    fn response_headers_drop_upstream_secrets_and_stale_length() {
        let headers = sanitize_response_headers(
            vec![
                header("content-length", "123"),
                header("set-cookie", "secret=1"),
                header("x-request-id", "rid"),
            ],
            ResponseHeaderMode::EventStream,
        );
        assert!(headers.iter().all(|header| header.name != "content-length"));
        assert!(headers.iter().all(|header| header.name != "set-cookie"));
        assert!(headers.iter().any(|header| header.name == "x-request-id"));
        assert!(headers.iter().any(|header| {
            header.name == "content-type" && header.value.starts_with(b"text/event-stream")
        }));
        assert!(headers.iter().any(|header| {
            header.name == "connection" && header.value.as_slice() == b"keep-alive"
        }));
    }

    #[test]
    fn json_mode_replaces_upstream_event_stream_content_type() {
        let headers = sanitize_response_headers(
            vec![header("content-type", "text/event-stream")],
            ResponseHeaderMode::Json,
        );
        assert_eq!(headers.len(), 1);
        assert_eq!(headers[0].name, "content-type");
        assert_eq!(headers[0].value, b"application/json; charset=utf-8");
    }
}
