use std::sync::Arc;

use clickhouse::Client as ClickHouseClient;
use reqwest::Client;

use crate::{
    config::AppConfig,
    err::AppResult,
    infra::{
        db::{self, DbConnection, DbPool},
        email::EmailClient,
        http_client::{HttpClientProfile, HttpClients},
        redis::RedisConnection,
    },
    plugin::runtime::PluginRuntime,
    request_event::RequestEventPublisher,
};

/// 全局应用状态。
///
/// 集中持有配置、基础设施连接、隔离后的 HTTP client 集合以及请求事件发布端口。
/// 统一使用 Arc 包裹，避免在 Axum handler 和后台任务之间重复克隆大对象。
#[derive(Clone)]
pub struct AppState {
    inner: Arc<AppStateInner>,
}

struct AppStateInner {
    config: AppConfig,
    db_pool: DbPool,
    redis: RedisConnection,
    email_client: EmailClient,
    http_clients: HttpClients,
    plugin_runtime: PluginRuntime,
    clickhouse: ClickHouseClient,
    request_events: RequestEventPublisher,
}

impl AppState {
    pub fn new(
        config: AppConfig,
        db_pool: DbPool,
        redis: RedisConnection,
        clickhouse: ClickHouseClient,
        http_clients: HttpClients,
        request_events: RequestEventPublisher,
    ) -> AppResult<Self> {
        let email_client = EmailClient::new(&config)?;
        let plugin_runtime = PluginRuntime::new()?;

        Ok(Self {
            inner: Arc::new(AppStateInner {
                config,
                db_pool,
                redis,
                email_client,
                http_clients,
                plugin_runtime,
                clickhouse,
                request_events,
            }),
        })
    }

    pub fn config(&self) -> &AppConfig {
        &self.inner.config
    }

    /// 获取 PostgreSQL 数据库连接。
    ///
    /// 对外只暴露连接获取能力，错误映射和日志由 infra::db 统一处理，
    /// 避免业务模块直接依赖连接池细节。
    pub async fn db_conn(&self) -> AppResult<DbConnection> {
        db::get_connection(&self.inner.db_pool).await
    }

    pub fn redis(&self) -> RedisConnection {
        self.inner.redis.clone()
    }

    pub fn email_client(&self) -> &EmailClient {
        &self.inner.email_client
    }

    /// 获取不携带任何 provider Cookie 状态的通用短请求 client。
    pub fn http_client(&self) -> &Client {
        self.inner.http_clients.buffered(HttpClientProfile::Generic)
    }

    /// 获取仅服务于 ChatGPT/Codex 账号域名的短请求 client。
    pub fn chatgpt_codex_http_client(&self) -> &Client {
        self.inner
            .http_clients
            .buffered(HttpClientProfile::ChatGptCodex)
    }

    /// 按 provider adapter 声明的 profile 选择长连接 client。
    pub fn streaming_http_client(&self, profile: HttpClientProfile) -> &Client {
        self.inner.http_clients.streaming(profile)
    }

    pub fn plugin_runtime(&self) -> &PluginRuntime {
        &self.inner.plugin_runtime
    }

    /// 获取核心请求事实的非阻塞发布端口。
    ///
    /// 调用方只发布强类型事件，不感知 worker、队列或任何后台处理结果。
    pub fn request_events(&self) -> &RequestEventPublisher {
        &self.inner.request_events
    }

    /// Dashboard statistics 使用的 ClickHouse 只读客户端。
    ///
    /// 查询 SQL、结果 DTO 和访问控制继续由 `http::dash::statistics` 管理；AppState 仅提供
    /// 基础设施句柄，worker 不再承担读路径职责。
    pub fn clickhouse(&self) -> &ClickHouseClient {
        &self.inner.clickhouse
    }
}
