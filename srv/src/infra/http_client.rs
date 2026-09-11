use std::{sync::Arc, time::Duration};

use moka::future::Cache;
use reqwest::{Client, cookie::CookieStore};
use tracing::{debug, info, trace, warn};
use uuid::Uuid;

use crate::{
    config::AppConfig,
    err::{AppError, AppResult},
    infra::account_proxy::AccountProxy,
};

const ACCOUNT_CLIENT_CACHE_CAPACITY: u64 = 4096;
const ACCOUNT_CLIENT_CACHE_IDLE: Duration = Duration::from_secs(30 * 60);

/// 上游 HTTP client 的选择依据。账号 ID 来自已调度资源，不能取自请求 header。
///
/// 通用 client 不启用 Cookie；ChatGPT client 以账号资源 UUID 隔离 Cookie 和连接池。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpClientProfile {
    Generic,
    ChatGptCodex {
        account_id: Uuid,
        proxy: AccountProxy,
    },
}

impl HttpClientProfile {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Generic => "generic",
            Self::ChatGptCodex { .. } => "chatgpt_codex",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct AccountClientKey {
    account_id: Uuid,
    proxy: AccountProxy,
}

/// 两个 client 共用本账号的 jar，但保留短请求与流式请求各自的超时策略。
/// Client 的 clone 复用内部连接池及 jar，不会重新创建 Cookie 状态。
#[derive(Clone)]
struct AccountHttpClients {
    buffered: Client,
    streaming: Client,
}

/// 进程内复用的 HTTP client 集合。
///
/// 账号组按 UUID 与规范化代理配置缓存；代理认证参与缓存键，禁止记录原始键。
/// 账号 token 不进入缓存，请求级 Authorization、UA 和显式 Cookie 由调用方设置。
/// 缓存淘汰只释放缓存持有的 client，已发出的请求继续使用自己的引用；后续缓存未命中时
/// 重新创建空 jar。缓存的容量和未取用时间不等同于在途请求数及 Cookie 自身有效期。
pub struct HttpClients {
    generic: Client,
    generic_streaming: Client,
    accounts: Cache<AccountClientKey, AccountHttpClients>,
    request_timeout: Duration,
    build_account: Arc<dyn Fn(AccountProxy) -> reqwest::Result<AccountHttpClients> + Send + Sync>,
}

impl HttpClients {
    pub fn build<C>(config: &AppConfig, cookie_store_factory: fn() -> Arc<C>) -> AppResult<Self>
    where
        C: CookieStore + 'static,
    {
        let request_timeout = Duration::from_secs(config.provider_upstream_timeout_seconds.max(1));
        let connect_timeout =
            Duration::from_secs(config.provider_upstream_connect_timeout_seconds.max(1));

        let generic = Client::builder()
            .timeout(request_timeout)
            .build()
            .map_err(|source| AppError::HttpClientBuild {
                message: format!("通用短请求 client: {source}"),
            })?;
        let generic_streaming = Client::builder()
            .connect_timeout(connect_timeout)
            .build()
            .map_err(|source| AppError::HttpClientBuild {
                message: format!("通用流式 client: {source}"),
            })?;

        let accounts = Cache::builder()
            .name("chatgpt_account_http_clients")
            .max_capacity(ACCOUNT_CLIENT_CACHE_CAPACITY)
            .time_to_idle(ACCOUNT_CLIENT_CACHE_IDLE)
            .eviction_listener(|key: Arc<AccountClientKey>, _clients, cause| {
                debug!(
                    account_id = %key.account_id,
                    reason = ?cause,
                    "账号 HTTP client 组已移出缓存，在途请求保留自身引用"
                );
            })
            .build();
        let build_account = Arc::new(move |proxy: AccountProxy| {
            // 必须在每次账号组初始化内部创建 jar，不能捕获一个跨账号共享的 store。
            let cookie_store = cookie_store_factory();
            let buffered = proxy
                .apply(Client::builder().timeout(request_timeout))?
                .cookie_provider(cookie_store.clone())
                .build()?;
            let streaming = proxy
                .apply(Client::builder().connect_timeout(connect_timeout))?
                .cookie_provider(cookie_store)
                .build()?;
            Ok(AccountHttpClients {
                buffered,
                streaming,
            })
        });

        info!(
            request_timeout_seconds = request_timeout.as_secs(),
            connect_timeout_seconds = connect_timeout.as_secs(),
            account_cache_capacity = ACCOUNT_CLIENT_CACHE_CAPACITY,
            account_cache_idle_seconds = ACCOUNT_CLIENT_CACHE_IDLE.as_secs(),
            "通用 HTTP client 与按账号隔离的 ChatGPT client 缓存已初始化"
        );

        Ok(Self {
            generic,
            generic_streaming,
            accounts,
            build_account,
            request_timeout,
        })
    }

    /// 导入前尚无账号 UUID，token 交换使用临时 client，代理规则与账号请求相同。
    pub async fn oauth_client(&self, proxy: AccountProxy) -> AppResult<Client> {
        let timeout = self.request_timeout;
        tokio::task::spawn_blocking(move || {
            proxy.apply(Client::builder().timeout(timeout))?.build()
        })
        .await
        .map_err(|_| AppError::HttpClientBuild {
            message: "OAuth client 构造任务失败".to_owned(),
        })?
        .map_err(|_| AppError::HttpClientBuild {
            message: "OAuth client 构造失败，请检查代理及 TLS 配置".to_owned(),
        })
    }

    pub fn generic(&self) -> &Client {
        &self.generic
    }

    pub async fn account_buffered(
        &self,
        account_id: Uuid,
        proxy: AccountProxy,
    ) -> AppResult<Client> {
        Ok(self.account_clients(account_id, proxy).await?.buffered)
    }

    pub async fn streaming(&self, profile: HttpClientProfile) -> AppResult<Client> {
        match profile {
            HttpClientProfile::Generic => Ok(self.generic_streaming.clone()),
            HttpClientProfile::ChatGptCodex { account_id, proxy } => {
                Ok(self.account_clients(account_id, proxy).await?.streaming)
            }
        }
    }

    async fn account_clients(
        &self,
        account_id: Uuid,
        proxy: AccountProxy,
    ) -> AppResult<AccountHttpClients> {
        // try_get_with 合并同一 UUID 与代理配置的并发初始化，失败不写缓存。构造 TLS client 放在
        // blocking 池中，避免账号冷启动阻塞 Tokio 的请求处理线程。
        let clients = self
            .accounts
            .try_get_with(
                AccountClientKey {
                    account_id,
                    proxy: proxy.clone(),
                },
                async {
                    debug!(%account_id, proxy = ?proxy, "开始初始化账号出站客户端组");
                    let build = self.build_account.clone();
                    let clients = tokio::task::spawn_blocking(move || build(proxy))
                        .await
                        .map_err(|source| AppError::HttpClientBuild {
                            message: format!("账号 client 构造任务失败: {source}"),
                        })?
                        .map_err(|source| AppError::HttpClientBuild {
                            message: format!("账号 client 构造失败: {source}"),
                        })?;
                    debug!(
                        %account_id,
                        "已创建账号 HTTP client 组：短请求与流式请求共享独立的受限 Cookie jar"
                    );
                    Ok::<_, AppError>(clients)
                },
            )
            .await
            .map_err(|source| {
                warn!(%account_id, error = %source, "初始化账号 HTTP client 失败");
                AppError::HttpClientBuild {
                    message: format!("account_id={account_id}: {source}"),
                }
            })?;
        trace!(%account_id, "已获取账号 HTTP client 组");
        Ok(clients)
    }
}
