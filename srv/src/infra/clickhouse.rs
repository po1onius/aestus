use clickhouse::{Client, sql::Identifier};
use tracing::{error, info};

use crate::{
    config::AppConfig,
    err::{AppError, AppResult},
};

/// 构建 ClickHouse HTTP 客户端。
///
/// request log 是时序追加数据，使用官方 ClickHouse Rust client 直接写入。这里不在代码里
/// 探测 ClickHouse 是否存在；如果启动环境缺少 ClickHouse，应按部署配置启动服务并创建表。
pub fn build_client(config: &AppConfig) -> Client {
    let client = Client::default()
        .with_url(config.clickhouse_url.clone())
        .with_database(config.clickhouse_database.clone())
        .with_user(config.clickhouse_user.clone())
        .with_password(config.clickhouse_password.clone());

    info!(
        clickhouse_url = %config.clickhouse_url,
        clickhouse_database = %config.clickhouse_database,
        clickhouse_user = %config.clickhouse_user,
        request_log_table = %config.request_log_table,
        request_usage_daily_table = %config.request_usage_daily_table,
        request_log_retention_days = config.request_log_retention_days.get(),
        service_timezone = %config.service_timezone,
        clickhouse_password_configured = true,
        "ClickHouse 客户端初始化完成"
    );

    client
}

/// 将服务配置的明细保留期同步为 ClickHouse 表 TTL。
///
/// 初始化 SQL 使用 30 天默认值，运行时再通过该语句覆盖，因此修改环境变量并重启服务即可
/// 生效。表不存在或当前账号没有 ALTER 权限属于部署错误，直接阻止服务启动并保留原始诊断。
pub async fn configure_request_log_retention(client: &Client, config: &AppConfig) -> AppResult<()> {
    let retention_days = config.request_log_retention_days.get();
    let result = client
        .query(
            "ALTER TABLE ? \
             MODIFY TTL request_started_at + toIntervalDay(?) DELETE",
        )
        .bind(Identifier(config.request_log_table.as_str()))
        .bind(retention_days)
        .execute()
        .await;

    if let Err(source) = result {
        error!(
            error = %source,
            clickhouse_table = %config.request_log_table,
            retention_days,
            "同步 ClickHouse 请求日志 TTL 失败"
        );
        return Err(AppError::Startup {
            message: format!(
                "同步 ClickHouse 表 {} 的请求日志 TTL 失败: {source}",
                config.request_log_table
            ),
        });
    }

    info!(
        clickhouse_table = %config.request_log_table,
        retention_days,
        "ClickHouse 请求日志 TTL 已与服务配置同步"
    );
    Ok(())
}
