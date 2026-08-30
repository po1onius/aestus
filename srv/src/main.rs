mod api_key;
mod config;
mod err;
mod gateway_auth;
mod http;
mod infra;
mod model;
mod plugin;
mod provider;
mod request_body;
mod request_event;
mod state;
mod user;
mod worker;

use std::process::ExitCode;
use tokio::net::TcpListener;
use tracing::{error, info};

use crate::{
    config::AppConfig,
    infra::{clickhouse, db, http_client::HttpClients, logging, redis},
    state::AppState,
};

#[tokio::main]
async fn main() -> ExitCode {
    let _logging_guards = match logging::init() {
        Ok(guards) => guards,
        Err(error) => {
            // tracing 尚未初始化时只能写 stderr；这是启动环境问题，应修正日志目录权限或配置。
            eprintln!("日志系统初始化失败: {error}");
            return ExitCode::FAILURE;
        }
    };

    if let Err(error) = run().await {
        error!(%error, "服务启动失败");
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}

async fn run() -> err::AppResult<()> {
    let config = AppConfig::from_env()?;
    let bind_addr = config.bind_addr;
    let db_pool = db::build_pool(&config)?;
    let redis = redis::build_connection(&config).await?;
    let clickhouse = clickhouse::build_client(&config);
    // 请求日志表由部署初始化脚本创建；服务在启动 worker 前同步可配置 TTL，确保任何新写入
    // 都遵循当前保留策略。同步失败代表 ClickHouse 表或权限配置错误，必须终止启动。
    clickhouse::configure_request_log_retention(&clickhouse, &config).await?;
    // 组合根显式登记 ChatGPT Codex 专用 cookie store。通用 HTTP client 不感知 GPT，
    // provider 只有在访问 ChatGPT/Codex 账号上游时才会选择对应 client profile。
    let http_clients = HttpClients::build(
        &config,
        provider::gpt::codex_http::header::cloudflare_cookie_store(),
    )?;
    // worker 在组合根显式启动；AppState 只取得不可反向控制 worker 的事件发布端口。
    let (request_events, _worker_runtime) = worker::start(
        db_pool.clone(),
        clickhouse.clone(),
        config.request_log_table.clone(),
        config.service_timezone,
    );
    let state = AppState::new(
        config,
        db_pool,
        redis,
        clickhouse,
        http_clients,
        request_events,
    )?;
    user::bootstrap_admin(&state).await?;
    // 任务集合必须覆盖 HTTP 服务的完整生命周期；退出或后续启动步骤失败时会统一停止
    // 所有 provider maintenance 循环，避免 JoinHandle 被丢弃后转为 detached 任务。
    let _provider_tasks = provider::start(&state).await?;
    let listener = TcpListener::bind(bind_addr)
        .await
        .map_err(|source| err::AppError::Startup {
            message: format!("监听地址 {bind_addr} 失败: {source}"),
        })?;

    info!(
        %bind_addr,
        service_timezone = %state.config().service_timezone,
        request_log_retention_days = state.config().request_log_retention_days.get(),
        "aestus gateway 启动完成"
    );
    let app = http::build_router(state);

    axum::serve(listener, app)
        .await
        .map_err(|source| err::AppError::Startup {
            message: format!("HTTP 服务异常退出: {source}"),
        })
}
