use redis::{AsyncCommands, aio::ConnectionManager};
use tracing::info;

use crate::{
    config::AppConfig,
    err::{AppError, AppResult},
};

pub type RedisConnection = ConnectionManager;

/// 创建 Redis 连接管理器。
///
/// Redis 是账号调度运行态的核心依赖。启动阶段会执行一次 PING，提前暴露
/// Redis 未启动、地址错误或认证失败等环境问题。
pub async fn build_connection(config: &AppConfig) -> AppResult<RedisConnection> {
    let client =
        redis::Client::open(config.redis_url.as_str()).map_err(|source| AppError::RedisClient {
            message: source.to_string(),
        })?;

    let mut connection =
        client
            .get_connection_manager()
            .await
            .map_err(|source| AppError::Redis {
                message: source.to_string(),
            })?;

    let pong: String = connection.ping().await.map_err(|source| AppError::Redis {
        message: source.to_string(),
    })?;

    info!(redis_ping = %pong, "Redis 连接初始化完成");

    Ok(connection)
}
