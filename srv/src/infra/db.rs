use diesel_async::{
    AsyncPgConnection,
    pooled_connection::{
        AsyncDieselConnectionManager,
        deadpool::{Object, Pool},
    },
};
use tracing::{error, info};

use crate::{
    config::AppConfig,
    err::{AppError, AppResult},
};

pub type DbPool = Pool<AsyncPgConnection>;
pub type DbConnection = Object<AsyncPgConnection>;

/// 创建 PostgreSQL 连接池。
///
/// 如果这里因为 PostgreSQL 未启动、账号密码错误或 DATABASE_URL 缺失而失败，
/// 应该调整运行环境，而不是在代码里降级到其他存储。
pub fn build_pool(config: &AppConfig) -> AppResult<DbPool> {
    let manager = AsyncDieselConnectionManager::<AsyncPgConnection>::new(&config.database_url);
    let pool = Pool::builder(manager)
        .max_size(config.database_pool_size)
        .build()
        .map_err(|source| AppError::DbPoolBuild {
            message: source.to_string(),
        })?;

    info!(
        database_pool_size = config.database_pool_size,
        "PostgreSQL 连接池初始化完成"
    );

    Ok(pool)
}

/// 从 PostgreSQL 连接池获取一个连接。
///
/// 业务代码统一通过这个入口拿连接，避免在每个 handler/service 里重复书写
/// pool.get().await 和错误映射逻辑；连接归还仍由 deadpool 的 Drop 自动完成。
pub async fn get_connection(pool: &DbPool) -> AppResult<DbConnection> {
    pool.get().await.map_err(|source| {
        error!(error = %source, "从 PostgreSQL 连接池获取连接失败");
        AppError::DbPoolGet {
            message: source.to_string(),
        }
    })
}
