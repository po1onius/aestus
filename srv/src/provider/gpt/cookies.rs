//! GPT 账号的受限 HTTP Cookie store，白名单与 Codex CLI 保持一致。

use std::sync::Arc;

use reqwest::{
    Url,
    cookie::{CookieStore, Jar},
    header::HeaderValue,
};

/// 为一个账号客户端组创建全新的 jar；调用方负责按账号资源 ID 复用客户端组。
pub(crate) fn account_cookie_store() -> Arc<ChatGptInfrastructureCookieStore> {
    Arc::new(ChatGptInfrastructureCookieStore::default())
}

/// ChatGPT 基础设施 cookie 存储。
///
/// 官方 Codex 客户端会在 reqwest client 上挂一个进程内 cookie jar，但只允许
/// Cloudflare 基础设施 cookie 和 OpenAI 的 `__oailb` 路由 cookie。
/// 每个账号客户端组拥有独立 jar，同一账号的短请求与流式请求共享。
/// 即使已按账号隔离，也只保存基础设施 Cookie，不接受 ChatGPT session 或 auth token。
#[derive(Debug, Default)]
pub(crate) struct ChatGptInfrastructureCookieStore {
    jar: Jar,
}

impl CookieStore for ChatGptInfrastructureCookieStore {
    fn set_cookies(&self, cookie_headers: &mut dyn Iterator<Item = &HeaderValue>, url: &Url) {
        if !is_chatgpt_cookie_url(url) {
            return;
        }

        let mut cloudflare_cookie_headers =
            cookie_headers.filter(|header| is_allowed_cloudflare_set_cookie_header(header));
        self.jar.set_cookies(&mut cloudflare_cookie_headers, url);
    }

    fn cookies(&self, url: &Url) -> Option<HeaderValue> {
        if !is_chatgpt_cookie_url(url) {
            return None;
        }

        self.jar.cookies(url).and_then(only_cloudflare_cookies)
    }
}

fn is_chatgpt_cookie_url(url: &Url) -> bool {
    if url.scheme() != "https" {
        return false;
    }

    let Some(host) = url.host_str() else {
        return false;
    };

    is_allowed_chatgpt_host(host)
}

fn is_allowed_chatgpt_host(host: &str) -> bool {
    matches!(
        host,
        "chatgpt.com" | "chat.openai.com" | "chatgpt-staging.com"
    ) || host.ends_with(".chatgpt.com")
        || host.ends_with(".chatgpt-staging.com")
}

fn is_allowed_cloudflare_set_cookie_header(header: &HeaderValue) -> bool {
    header
        .to_str()
        .ok()
        .and_then(set_cookie_name)
        .is_some_and(is_allowed_cloudflare_cookie_name)
}

fn set_cookie_name(header: &str) -> Option<&str> {
    let (name, _) = header.split_once('=')?;
    let name = name.trim();
    (!name.is_empty()).then_some(name)
}

fn only_cloudflare_cookies(header: HeaderValue) -> Option<HeaderValue> {
    let header = header.to_str().ok()?;
    let cookies = header
        .split(';')
        .filter_map(|cookie| {
            let cookie = cookie.trim();
            let name = cookie.split_once('=')?.0.trim();
            is_allowed_cloudflare_cookie_name(name).then_some(cookie)
        })
        .collect::<Vec<_>>()
        .join("; ");

    if cookies.is_empty() {
        None
    } else {
        HeaderValue::from_str(&cookies).ok()
    }
}

fn is_allowed_cloudflare_cookie_name(name: &str) -> bool {
    // 与 Codex CLI 的 HTTP cookie store 保持一致：__oailb 是 OpenAI 基础设施
    // 路由 cookie，不是认证 cookie，因此允许与 Cloudflare cookie 一同共享。
    matches!(
        name,
        "__cf_bm"
            | "__cflb"
            | "__cfruid"
            | "__cfseq"
            | "__cfwaitingroom"
            | "__oailb"
            | "_cfuvid"
            | "cf_clearance"
            | "cf_ob_info"
            | "cf_use_ob"
    ) || name.starts_with("cf_chl_")
}
