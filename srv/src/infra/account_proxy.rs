//! 账号出站代理。控制台保存原始 URL，连接与缓存键使用规范化配置，Debug 不输出认证信息。
use std::fmt;

use reqwest::{ClientBuilder, Proxy, Url};
use serde::{Deserialize, Serialize};

use crate::err::{AppError, AppResult};

#[derive(Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AccountProxy(Option<String>);

#[derive(Debug, Serialize)]
pub struct ProxyView {
    pub url: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProxyUpdate {
    pub proxy_url: Option<String>,
}

/// 校验配置并保留输入格式（仅去除首尾空白），供持久化和控制台回显。
pub fn validate_proxy_url(raw: Option<&str>) -> AppResult<Option<String>> {
    AccountProxy::parse(raw)?;
    Ok(raw
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .map(str::to_owned))
}

impl fmt::Debug for AccountProxy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.0.is_some() {
            "AccountProxy(configured)"
        } else {
            "AccountProxy(direct)"
        })
    }
}

impl AccountProxy {
    /// URL 校验不回显原始输入，避免非法代理地址中的密码进入日志或错误响应。
    pub fn parse(raw: Option<&str>) -> AppResult<Self> {
        let Some(raw) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
            return Ok(Self::default());
        };
        if raw.len() > 4096 {
            return Err(invalid("代理 URL 不能超过 4096 字节"));
        }
        let mut url = Url::parse(raw).map_err(|_| invalid("代理 URL 格式无效"))?;
        if !matches!(url.scheme(), "http" | "https" | "socks5" | "socks5h") {
            return Err(invalid("代理协议仅支持 http、https、socks5 和 socks5h"));
        }
        if url.host_str().is_none() || url.port() == Some(0) {
            return Err(invalid("代理 URL 必须包含有效主机和端口"));
        }
        if !matches!(url.path(), "" | "/") || url.query().is_some() || url.fragment().is_some() {
            return Err(invalid("代理 URL 不能包含路径、查询参数或 fragment"));
        }
        // SOCKS URL 的空路径与单个斜杠表示同一代理，统一后再作为缓存键。
        url.set_path("");
        if url.scheme().starts_with("socks5") && url.port().is_none() {
            url.set_port(Some(1080))
                .map_err(|_| invalid("代理端口无效"))?;
        }
        Proxy::all(url.as_str()).map_err(|_| invalid("代理 URL 或认证信息无效"))?;
        Ok(Self(Some(url.to_string())))
    }

    /// 无代理时明确直连；有代理时只使用指定代理，不受环境代理及 NO_PROXY 干扰。
    pub fn apply(&self, builder: ClientBuilder) -> reqwest::Result<ClientBuilder> {
        let builder = builder.no_proxy();
        match &self.0 {
            Some(url) => Ok(builder.proxy(Proxy::all(url)?)),
            None => Ok(builder),
        }
    }
}

fn invalid(message: &str) -> AppError {
    AppError::BadRequest {
        message: message.to_owned(),
    }
}
