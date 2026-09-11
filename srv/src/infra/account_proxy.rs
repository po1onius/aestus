//! 账号出站代理。持久值和缓存键包含认证信息，Debug 与控制台视图始终脱敏。
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
    pub has_auth: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProxyUpdate {
    pub proxy_url: Option<String>,
    #[serde(default)]
    pub keep_auth: bool,
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

    pub fn stored_url(&self) -> Option<String> {
        self.0.clone()
    }

    pub fn view(&self) -> Option<ProxyView> {
        let mut url = Url::parse(self.0.as_deref()?).ok()?;
        let has_auth = !url.username().is_empty() || url.password().is_some();
        let _ = url.set_username("");
        let _ = url.set_password(None);
        Some(ProxyView {
            url: url.to_string(),
            has_auth,
        })
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

impl ProxyUpdate {
    /// 调用方须在账号行锁内传入最新配置，避免保留认证时读到并发更新前的密码。
    pub fn resolve(&self, current: &AccountProxy) -> AppResult<AccountProxy> {
        let mut next = AccountProxy::parse(self.proxy_url.as_deref())?;
        if self.keep_auth
            && let Some(next_url) = next.0.as_mut()
        {
            let mut url = Url::parse(next_url).map_err(|_| invalid("代理 URL 格式无效"))?;
            if !url.username().is_empty() || url.password().is_some() {
                return Err(invalid("保留现有认证时，请填写不含用户名和密码的代理 URL"));
            }
            if let Some(current) = current.0.as_deref() {
                let old = Url::parse(current).map_err(|_| invalid("已保存的代理 URL 无效"))?;
                url.set_username(old.username())
                    .map_err(|_| invalid("代理用户名无效"))?;
                url.set_password(old.password())
                    .map_err(|_| invalid("代理密码无效"))?;
            }
            *next_url = url.to_string();
        }
        AccountProxy::parse(next.0.as_deref())
    }
}

fn invalid(message: &str) -> AppError {
    AppError::BadRequest {
        message: message.to_owned(),
    }
}
