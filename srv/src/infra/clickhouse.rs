use clickhouse::Client;
use tracing::info;

use crate::config::AppConfig;

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
        clickhouse_password_configured = true,
        "ClickHouse 客户端初始化完成"
    );

    client
}
