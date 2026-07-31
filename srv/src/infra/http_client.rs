use std::sync::Arc;

use reqwest::{Client, cookie::CookieStore};
use tracing::info;

use crate::{
    config::AppConfig,
    err::{AppError, AppResult},
};

/// 上游请求使用的 HTTP client 配置档案。
///
/// 通用 client 不启用 cookie store，避免某个 provider 的状态污染其他厂商请求。
/// `ChatGptCodex` 仅用于 ChatGPT/Codex 域名，并由调用方提供只保存 Cloudflare
/// 基础设施 Cookie 的受限 store。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpClientProfile {
    Generic,
    ChatGptCodex,
}

impl HttpClientProfile {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Generic => "generic",
            Self::ChatGptCodex => "chatgpt_codex",
        }
    }
}

/// 进程内复用的 reqwest client 集合。
///
/// buffered client 带整体请求超时，适用于 OAuth、探活和短响应接口；streaming client
/// 只限制建连，响应头超时和响应体读取策略由通用 provider pipeline 分阶段控制。
pub struct HttpClients {
    generic: Client,
    generic_streaming: Client,
    chatgpt_codex: Client,
    chatgpt_codex_streaming: Client,
}

impl HttpClients {
    pub fn build<C>(config: &AppConfig, chatgpt_cookie_store: Arc<C>) -> AppResult<Self>
    where
        C: CookieStore + 'static,
    {
        let request_timeout =
            std::time::Duration::from_secs(config.provider_upstream_timeout_seconds.max(1));
        let connect_timeout =
            std::time::Duration::from_secs(config.provider_upstream_connect_timeout_seconds.max(1));

        let generic = Client::builder()
            .timeout(request_timeout)
            .build()
            .map_err(|source| client_build_error(HttpClientProfile::Generic, false, source))?;
        let generic_streaming = Client::builder()
            .connect_timeout(connect_timeout)
            .build()
            .map_err(|source| client_build_error(HttpClientProfile::Generic, true, source))?;
        // 两个 ChatGPT client 共用同一个受限 cookie store，使 buffered 额度查询和
        // streaming Codex 请求能够观察同一组 Cloudflare 基础设施 Cookie。UA 与
        // originator 在 GPT provider 请求级构造，避免 client 默认值削弱资源 override。
        let chatgpt_codex = Client::builder()
            .timeout(request_timeout)
            .cookie_provider(chatgpt_cookie_store.clone())
            .build()
            .map_err(|source| client_build_error(HttpClientProfile::ChatGptCodex, false, source))?;
        let chatgpt_codex_streaming = Client::builder()
            .connect_timeout(connect_timeout)
            .cookie_provider(chatgpt_cookie_store)
            .build()
            .map_err(|source| client_build_error(HttpClientProfile::ChatGptCodex, true, source))?;

        info!(
            request_timeout_seconds = config.provider_upstream_timeout_seconds.max(1),
            connect_timeout_seconds = config.provider_upstream_connect_timeout_seconds.max(1),
            "通用与 ChatGPT Codex 专用 HTTP client 已完成隔离初始化"
        );

        Ok(Self {
            generic,
            generic_streaming,
            chatgpt_codex,
            chatgpt_codex_streaming,
        })
    }

    pub fn buffered(&self, profile: HttpClientProfile) -> &Client {
        match profile {
            HttpClientProfile::Generic => &self.generic,
            HttpClientProfile::ChatGptCodex => &self.chatgpt_codex,
        }
    }

    pub fn streaming(&self, profile: HttpClientProfile) -> &Client {
        match profile {
            HttpClientProfile::Generic => &self.generic_streaming,
            HttpClientProfile::ChatGptCodex => &self.chatgpt_codex_streaming,
        }
    }
}

fn client_build_error(
    profile: HttpClientProfile,
    streaming: bool,
    source: reqwest::Error,
) -> AppError {
    AppError::Startup {
        message: format!(
            "构建 HTTP client 失败: profile={}, streaming={streaming}, error={source}",
            profile.as_str()
        ),
    }
}
