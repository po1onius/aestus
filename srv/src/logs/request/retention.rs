//! 请求明细的存储保留期与可查询日期范围。

use std::num::NonZeroU32;

use chrono::{DateTime, Days, NaiveDate, Utc};
use clickhouse::{Client, sql::Identifier};
use tracing::{error, info};

use crate::{
    err::{AppError, AppResult},
    logs::calendar::{current_service_date, local_day_range_utc},
};

pub(crate) struct RequestLogDateRange {
    pub date: NaiveDate,
    pub start_at: DateTime<Utc>,
    pub end_at: DateTime<Utc>,
}

/// 将服务配置的明细保留期同步为 ClickHouse 表 TTL。
///
/// 初始化 SQL 使用 30 天默认值，运行时再通过该语句覆盖，因此修改环境变量并重启服务即可
/// 生效。表不存在或当前账号没有 ALTER 权限属于部署错误，直接阻止服务启动并保留原始诊断。
pub(crate) async fn configure_retention(
    client: &Client,
    table: &str,
    retention_days: NonZeroU32,
) -> AppResult<()> {
    let retention_days = retention_days.get();
    let result = client
        .query(
            "ALTER TABLE ? \
             MODIFY TTL request_started_at + toIntervalDay(?) DELETE",
        )
        .bind(Identifier(table))
        .bind(retention_days)
        .execute()
        .await;

    if let Err(source) = result {
        error!(
            error = %source,
            clickhouse_table = %table,
            retention_days,
            "同步 ClickHouse 请求日志 TTL 失败"
        );
        return Err(AppError::Startup {
            message: format!("同步 ClickHouse 表 {} 的请求日志 TTL 失败: {source}", table),
        });
    }

    info!(
        clickhouse_table = %table,
        retention_days,
        "ClickHouse 请求日志 TTL 已与服务配置同步"
    );
    Ok(())
}

pub(crate) fn normalize_log_date(
    date: Option<NaiveDate>,
    timezone: chrono_tz::Tz,
    retention_days: NonZeroU32,
) -> AppResult<RequestLogDateRange> {
    let today = current_service_date(timezone);
    // 今天与前 retention_days - 1 个完整自然日始终落在滚动 TTL 内；更早的日期可能已被
    // ClickHouse 后台 merge 部分或全部清理，因此直接拒绝而不返回容易误解的空结果。
    let earliest_date = today
        .checked_sub_days(Days::new(u64::from(retention_days.get() - 1)))
        .ok_or_else(|| AppError::BadRequest {
            message: "计算请求日志最早可查日期时超出支持范围".to_owned(),
        })?;
    let date = date.unwrap_or(today);
    if date < earliest_date || date > today {
        return Err(AppError::BadRequest {
            message: format!(
                "请求日志日期必须在 {earliest_date} 到 {today} 之间（保留 {} 天，服务时区 {timezone}）",
                retention_days.get()
            ),
        });
    }

    let (start_at, end_at) = local_day_range_utc(timezone, date)?;
    Ok(RequestLogDateRange {
        date,
        start_at,
        end_at,
    })
}
