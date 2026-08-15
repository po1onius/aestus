//! Dashboard token 用量聚合接口。
//!
//! 普通用户只统计自己的余额与请求日志，admin 统计 PostgreSQL 中全部用户的余额及
//! ClickHouse 中全部已归属用户的请求日志。额度属于附属能力，允许极端故障下少量统计
//! 缺失，因此这里不引入账单流水或跨库事务。用量概览固定统计调用方时区下包含今天的
//! 最近 365 个自然日，并一次返回总量、分布与每日明细。

use std::collections::HashMap;

use axum::{
    Json, Router,
    extract::{Query, State},
    routing::get,
};
use chrono::{DateTime, Datelike, Days, LocalResult, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use clickhouse::{Row, sql::Identifier};
use serde::{Deserialize, Serialize};
use tracing::{error, info};
use uuid::Uuid;

use crate::{
    err::{AppError, AppResult},
    http::dash::auth,
    model::User,
    state::AppState,
    user,
};

const MAX_TIMEZONE_BYTES: usize = 64;
const USAGE_YEAR_DAYS: u64 = 365;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UsageQuery {
    timezone: Option<String>,
}

#[derive(Debug)]
struct NormalizedUsageRange {
    start_at: DateTime<Utc>,
    end_at: DateTime<Utc>,
    start_date: NaiveDate,
    end_date: NaiveDate,
    timezone: String,
}

/// 用量查询的数据边界由已认证用户角色唯一决定，不接受调用方通过 query 参数自行扩大。
#[derive(Debug, Clone, Copy)]
enum UsageScope {
    CurrentUser(Uuid),
    AllUsers,
}

impl UsageScope {
    fn from_user(user: &User) -> Self {
        if user.is_admin() {
            Self::AllUsers
        } else {
            Self::CurrentUser(user.id)
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::CurrentUser(_) => "current_user",
            Self::AllUsers => "all_users",
        }
    }

    fn user_id(self) -> Option<Uuid> {
        match self {
            Self::CurrentUser(user_id) => Some(user_id),
            Self::AllUsers => None,
        }
    }
}

#[derive(Debug)]
struct UsageUserDirectory {
    remaining_tokens: String,
    consumed_tokens: String,
    usernames_by_id: HashMap<Uuid, String>,
}

#[derive(Debug, Deserialize, Row)]
struct UsageTotalsRow {
    total_tokens: String,
    request_count: u64,
}

#[derive(Debug, Deserialize, Row)]
struct UsageDailyRow {
    usage_date: String,
    total_tokens_text: String,
    request_count: u64,
}

#[derive(Debug, Deserialize, Row)]
struct UsageModelRow {
    provider: String,
    model: String,
    total_tokens_text: String,
    request_count: u64,
}

#[derive(Debug, Deserialize, Row)]
struct UsageApiKeyRow {
    api_key_name_text: String,
    total_tokens_text: String,
    request_count: u64,
}

#[derive(Debug, Deserialize, Row)]
struct UsageUserRow {
    #[serde(with = "clickhouse::serde::uuid")]
    user_id: Uuid,
    username: String,
    total_tokens_text: String,
    request_count: u64,
}

#[derive(Debug)]
struct UsageBreakdownRows {
    api_keys: Vec<UsageApiKeyRow>,
    users: Vec<UsageUserRow>,
}

#[derive(Debug, Serialize)]
struct UsageOverviewResponse {
    scope: &'static str,
    remaining_tokens: String,
    consumed_tokens: String,
    start_at: DateTime<Utc>,
    end_at: DateTime<Utc>,
    timezone: String,
    period: UsagePeriodResponse,
    daily: Vec<UsageDailyPoint>,
    models: Vec<UsageModelPoint>,
    api_keys: Vec<UsageApiKeyPoint>,
    users: Vec<UsageUserPoint>,
    generated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
struct UsagePeriodResponse {
    total_tokens: String,
    request_count: String,
}

#[derive(Debug, Serialize)]
struct UsageDailyPoint {
    date: NaiveDate,
    total_tokens: String,
    request_count: String,
}

#[derive(Debug, Serialize)]
struct UsageModelPoint {
    provider: String,
    model: String,
    total_tokens: String,
    request_count: String,
    percentage: f64,
}

#[derive(Debug, Serialize)]
struct UsageApiKeyPoint {
    name: String,
    total_tokens: String,
    request_count: String,
    percentage: f64,
}

#[derive(Debug, Serialize)]
struct UsageUserPoint {
    user_id: Uuid,
    username: String,
    total_tokens: String,
    request_count: String,
    percentage: f64,
}

pub(super) fn router() -> Router<AppState> {
    Router::new().route("/", get(get_usage))
}

async fn get_usage(
    State(state): State<AppState>,
    auth::CurrentUser(current_user): auth::CurrentUser,
    Query(query): Query<UsageQuery>,
) -> AppResult<Json<UsageOverviewResponse>> {
    let query = normalize_usage_range(query.timezone)?;
    let scope = UsageScope::from_user(&current_user);

    // PostgreSQL 用户快照、总量、每日聚合、模型分布与 Key/用户分布彼此独立，并行查询，
    // 避免固定一年窗口扩大后把 ClickHouse 查询串行叠加到 Dashboard 首屏延迟上。
    let (users, totals, daily_rows, model_rows, breakdown_rows) = tokio::try_join!(
        load_usage_user_directory(&state, &current_user, scope),
        query_usage_totals(&state, scope, &query),
        query_usage_daily(&state, scope, &query),
        query_usage_models(&state, scope, &query),
        query_usage_breakdown(&state, scope, &query),
    )?;
    let daily = build_daily_usage(daily_rows, &query)?;

    let period_total = parse_aggregate_for_percentage(&totals.total_tokens);
    let models = model_rows
        .into_iter()
        .map(|row| UsageModelPoint {
            percentage: token_percentage(&row.total_tokens_text, period_total),
            provider: row.provider,
            model: row.model,
            total_tokens: row.total_tokens_text,
            request_count: row.request_count.to_string(),
        })
        .collect::<Vec<_>>();
    let api_keys = breakdown_rows
        .api_keys
        .into_iter()
        .map(|row| UsageApiKeyPoint {
            percentage: token_percentage(&row.total_tokens_text, period_total),
            name: row.api_key_name_text,
            total_tokens: row.total_tokens_text,
            request_count: row.request_count.to_string(),
        })
        .collect::<Vec<_>>();
    let user_points = breakdown_rows
        .users
        .into_iter()
        .map(|row| UsageUserPoint {
            percentage: token_percentage(&row.total_tokens_text, period_total),
            username: if row.username.is_empty() {
                users
                    .usernames_by_id
                    .get(&row.user_id)
                    .cloned()
                    .unwrap_or_else(|| format!("未知用户 ({})", row.user_id))
            } else {
                row.username
            },
            user_id: row.user_id,
            total_tokens: row.total_tokens_text,
            request_count: row.request_count.to_string(),
        })
        .collect::<Vec<_>>();

    info!(
        actor_user_id = %current_user.id,
        usage_scope = scope.as_str(),
        scoped_user_id = ?scope.user_id(),
        start_at = %query.start_at,
        end_at = %query.end_at,
        timezone = %query.timezone,
        total_tokens = %totals.total_tokens,
        request_count = totals.request_count,
        daily_points = daily.len(),
        active_days = daily.iter().filter(|point| point.total_tokens != "0").count(),
        model_groups = models.len(),
        api_key_groups = api_keys.len(),
        user_groups = user_points.len(),
        "Dashboard token 用量概览聚合完成"
    );

    Ok(Json(UsageOverviewResponse {
        scope: scope.as_str(),
        remaining_tokens: users.remaining_tokens,
        consumed_tokens: users.consumed_tokens,
        start_at: query.start_at,
        end_at: query.end_at,
        timezone: query.timezone,
        period: UsagePeriodResponse {
            total_tokens: totals.total_tokens,
            request_count: totals.request_count.to_string(),
        },
        daily,
        models,
        api_keys,
        users: user_points,
        generated_at: Utc::now(),
    }))
}

fn normalize_usage_range(timezone: Option<String>) -> AppResult<NormalizedUsageRange> {
    let (timezone_name, timezone) = normalize_timezone(timezone)?;
    let today = Utc::now().with_timezone(&timezone).date_naive();
    let start_date = today
        .checked_sub_days(Days::new(USAGE_YEAR_DAYS - 1))
        .ok_or_else(|| AppError::BadRequest {
            message: "计算近一年用量开始日期时超出支持范围".to_owned(),
        })?;
    let end_date = today
        .checked_add_days(Days::new(1))
        .ok_or_else(|| AppError::BadRequest {
            message: "计算近一年用量结束日期时超出支持范围".to_owned(),
        })?;

    Ok(NormalizedUsageRange {
        start_at: local_day_start_utc(timezone, start_date)?,
        end_at: local_day_start_utc(timezone, end_date)?,
        start_date,
        end_date,
        timezone: timezone_name,
    })
}

/// 不再只校验字符串外形：解析为 chrono-tz 的真实时区，确保 Rust 计算出的自然日边界与
/// ClickHouse 按日分组使用完全相同的 IANA 名称。
fn normalize_timezone(timezone: Option<String>) -> AppResult<(String, Tz)> {
    let timezone = timezone.unwrap_or_else(|| "UTC".to_owned());
    let timezone = timezone.trim();
    if timezone.is_empty() || timezone.len() > MAX_TIMEZONE_BYTES {
        return Err(AppError::BadRequest {
            message: "timezone 必须是合法且不超过 64 字节的 IANA 时区名称".to_owned(),
        });
    }
    let parsed = timezone.parse::<Tz>().map_err(|_| AppError::BadRequest {
        message: format!("timezone 不是有效的 IANA 时区名称: {timezone}"),
    })?;
    Ok((timezone.to_owned(), parsed))
}

fn local_day_start_utc(timezone: Tz, date: NaiveDate) -> AppResult<DateTime<Utc>> {
    // 少数 IANA 时区会恰好在午夜向前切换，使 00:00 不存在。逐分钟寻找该日期第一次
    // 出现的本地时间，既能覆盖整点和半小时切换，也不会用固定 24 小时假设破坏自然日。
    for minute_of_day in 0..(24 * 60) {
        let hour = minute_of_day / 60;
        let minute = minute_of_day % 60;
        let local =
            timezone.with_ymd_and_hms(date.year(), date.month(), date.day(), hour, minute, 0);
        match local {
            LocalResult::Single(value) => return Ok(value.with_timezone(&Utc)),
            // 午夜回拨产生两个同名本地时刻时，取较早的绝对时间作为自然日开端。
            LocalResult::Ambiguous(first, second) => {
                return Ok(first.min(second).with_timezone(&Utc));
            }
            LocalResult::None => {}
        }
    }
    Err(AppError::BadRequest {
        message: format!("时区 {timezone} 中不存在本地日期 {date}"),
    })
}

async fn load_usage_user_directory(
    state: &AppState,
    current_user: &User,
    scope: UsageScope,
) -> AppResult<UsageUserDirectory> {
    if matches!(scope, UsageScope::CurrentUser(_)) {
        return Ok(UsageUserDirectory {
            remaining_tokens: current_user.quota.to_string(),
            consumed_tokens: current_user.consumed_tokens.to_string(),
            usernames_by_id: HashMap::from([(current_user.id, current_user.username.clone())]),
        });
    }

    let mut conn = state.db_conn().await?;
    let snapshots = user::list_usage_snapshots(&mut conn).await?;
    let mut remaining_tokens = 0_i128;
    let mut consumed_tokens = 0_i128;
    let mut usernames_by_id = HashMap::with_capacity(snapshots.len());

    for snapshot in snapshots {
        remaining_tokens = remaining_tokens
            .checked_add(i128::from(snapshot.quota))
            .ok_or_else(|| AppError::DbQuery {
                message: "汇总全部用户剩余 Token 时发生整数溢出".to_owned(),
            })?;
        consumed_tokens = consumed_tokens
            .checked_add(i128::from(snapshot.consumed_tokens))
            .ok_or_else(|| AppError::DbQuery {
                message: "汇总全部用户累计消耗时发生整数溢出".to_owned(),
            })?;
        usernames_by_id.insert(snapshot.id, snapshot.username);
    }

    info!(
        user_count = usernames_by_id.len(),
        remaining_tokens = %remaining_tokens,
        consumed_tokens = %consumed_tokens,
        "Dashboard 全部用户额度快照聚合完成"
    );
    Ok(UsageUserDirectory {
        remaining_tokens: remaining_tokens.to_string(),
        consumed_tokens: consumed_tokens.to_string(),
        usernames_by_id,
    })
}

async fn query_usage_totals(
    state: &AppState,
    scope: UsageScope,
    query: &NormalizedUsageRange,
) -> AppResult<UsageTotalsRow> {
    let current_user_sql = "SELECT \
        toString(sum(total_tokens)) AS total_tokens, \
        count() AS request_count \
        FROM ? \
        WHERE user_id = ? \
          AND request_started_at >= fromUnixTimestamp64Milli(?, 'UTC') \
          AND request_started_at < fromUnixTimestamp64Milli(?, 'UTC')";
    let all_users_sql = "SELECT \
        toString(sum(total_tokens)) AS total_tokens, \
        count() AS request_count \
        FROM ? \
        WHERE user_id IS NOT NULL \
          AND request_started_at >= fromUnixTimestamp64Milli(?, 'UTC') \
          AND request_started_at < fromUnixTimestamp64Milli(?, 'UTC')";

    let request = state
        .clickhouse()
        .query(match scope {
            UsageScope::CurrentUser(_) => current_user_sql,
            UsageScope::AllUsers => all_users_sql,
        })
        .bind(Identifier(state.config().request_log_table.as_str()));
    let request = match scope {
        UsageScope::CurrentUser(user_id) => request.bind(user_id),
        UsageScope::AllUsers => request,
    };
    request
        .bind(query.start_at.timestamp_millis())
        .bind(query.end_at.timestamp_millis())
        .fetch_one::<UsageTotalsRow>()
        .await
        .map_err(|source| usage_query_error("period_totals", scope, query, source))
}

async fn query_usage_daily(
    state: &AppState,
    scope: UsageScope,
    query: &NormalizedUsageRange,
) -> AppResult<Vec<UsageDailyRow>> {
    // 时间过滤仍使用 UTC 索引友好的原始列边界；分组日期显式转换到用户时区，确保夏令时
    // 切换日即使只有 23/25 小时也只形成一个正确的本地自然日。
    let current_user_sql = "SELECT \
            toString(toDate(request_started_at, ?)) AS usage_date, \
            toString(sum(total_tokens)) AS total_tokens_text, \
            count() AS request_count \
         FROM ? \
         WHERE user_id = ? \
           AND request_started_at >= fromUnixTimestamp64Milli(?, 'UTC') \
           AND request_started_at < fromUnixTimestamp64Milli(?, 'UTC') \
         GROUP BY usage_date \
         ORDER BY usage_date";
    let all_users_sql = "SELECT \
            toString(toDate(request_started_at, ?)) AS usage_date, \
            toString(sum(total_tokens)) AS total_tokens_text, \
            count() AS request_count \
         FROM ? \
         WHERE user_id IS NOT NULL \
           AND request_started_at >= fromUnixTimestamp64Milli(?, 'UTC') \
           AND request_started_at < fromUnixTimestamp64Milli(?, 'UTC') \
         GROUP BY usage_date \
         ORDER BY usage_date";

    let request = state
        .clickhouse()
        .query(match scope {
            UsageScope::CurrentUser(_) => current_user_sql,
            UsageScope::AllUsers => all_users_sql,
        })
        .bind(query.timezone.as_str())
        .bind(Identifier(state.config().request_log_table.as_str()));
    let request = match scope {
        UsageScope::CurrentUser(user_id) => request.bind(user_id),
        UsageScope::AllUsers => request,
    };
    request
        .bind(query.start_at.timestamp_millis())
        .bind(query.end_at.timestamp_millis())
        .fetch_all::<UsageDailyRow>()
        .await
        .map_err(|source| usage_query_error("daily", scope, query, source))
}

async fn query_usage_models(
    state: &AppState,
    scope: UsageScope,
    query: &NormalizedUsageRange,
) -> AppResult<Vec<UsageModelRow>> {
    // ClickHouse 的 SELECT 别名在 HAVING/ORDER BY 中可见。如果字符串结果也命名为
    // total_tokens，后面的 sum(total_tokens) 会被解析为对 String 别名再次聚合。内部使用
    // 独立别名，确保聚合表达式始终引用请求日志的 Int64 原始列。
    let current_user_sql = "SELECT \
        provider, \
        ifNull(model, '未记录') AS model, \
        toString(sum(total_tokens)) AS total_tokens_text, \
        count() AS request_count \
        FROM ? \
        WHERE user_id = ? \
          AND request_started_at >= fromUnixTimestamp64Milli(?, 'UTC') \
          AND request_started_at < fromUnixTimestamp64Milli(?, 'UTC') \
        GROUP BY provider, model \
        HAVING sum(total_tokens) > 0 \
        ORDER BY sum(total_tokens) DESC";
    let all_users_sql = "SELECT \
        provider, \
        ifNull(model, '未记录') AS model, \
        toString(sum(total_tokens)) AS total_tokens_text, \
        count() AS request_count \
        FROM ? \
        WHERE user_id IS NOT NULL \
          AND request_started_at >= fromUnixTimestamp64Milli(?, 'UTC') \
          AND request_started_at < fromUnixTimestamp64Milli(?, 'UTC') \
        GROUP BY provider, model \
        HAVING sum(total_tokens) > 0 \
        ORDER BY sum(total_tokens) DESC";

    let request = state
        .clickhouse()
        .query(match scope {
            UsageScope::CurrentUser(_) => current_user_sql,
            UsageScope::AllUsers => all_users_sql,
        })
        .bind(Identifier(state.config().request_log_table.as_str()));
    let request = match scope {
        UsageScope::CurrentUser(user_id) => request.bind(user_id),
        UsageScope::AllUsers => request,
    };
    request
        .bind(query.start_at.timestamp_millis())
        .bind(query.end_at.timestamp_millis())
        .fetch_all::<UsageModelRow>()
        .await
        .map_err(|source| usage_query_error("model_distribution", scope, query, source))
}

async fn query_usage_breakdown(
    state: &AppState,
    scope: UsageScope,
    query: &NormalizedUsageRange,
) -> AppResult<UsageBreakdownRows> {
    match scope {
        UsageScope::CurrentUser(user_id) => Ok(UsageBreakdownRows {
            api_keys: query_usage_api_keys(state, user_id, query).await?,
            users: Vec::new(),
        }),
        UsageScope::AllUsers => Ok(UsageBreakdownRows {
            api_keys: Vec::new(),
            users: query_usage_users(state, query).await?,
        }),
    }
}

async fn query_usage_api_keys(
    state: &AppState,
    user_id: Uuid,
    query: &NormalizedUsageRange,
) -> AppResult<Vec<UsageApiKeyRow>> {
    // API Key 名称在单个用户内唯一且不支持改名，普通用户视图可以直接按名称聚合。
    let sql = "SELECT \
        ifNull(api_key_name, '未记录') AS api_key_name_text, \
        toString(sum(total_tokens)) AS total_tokens_text, \
        count() AS request_count \
        FROM ? \
        WHERE user_id = ? \
          AND request_started_at >= fromUnixTimestamp64Milli(?, 'UTC') \
          AND request_started_at < fromUnixTimestamp64Milli(?, 'UTC') \
        GROUP BY api_key_name_text \
        HAVING sum(total_tokens) > 0 \
        ORDER BY sum(total_tokens) DESC";

    state
        .clickhouse()
        .query(sql)
        .bind(Identifier(state.config().request_log_table.as_str()))
        .bind(user_id)
        .bind(query.start_at.timestamp_millis())
        .bind(query.end_at.timestamp_millis())
        .fetch_all::<UsageApiKeyRow>()
        .await
        .map_err(|source| {
            usage_query_error(
                "api_key_distribution",
                UsageScope::CurrentUser(user_id),
                query,
                source,
            )
        })
}

async fn query_usage_users(
    state: &AppState,
    query: &NormalizedUsageRange,
) -> AppResult<Vec<UsageUserRow>> {
    let sql = "SELECT \
        assumeNotNull(user_id) AS user_id, \
        ifNull(any(username), '') AS username, \
        toString(sum(total_tokens)) AS total_tokens_text, \
        count() AS request_count \
        FROM ? \
        WHERE user_id IS NOT NULL \
          AND request_started_at >= fromUnixTimestamp64Milli(?, 'UTC') \
          AND request_started_at < fromUnixTimestamp64Milli(?, 'UTC') \
        GROUP BY user_id \
        HAVING sum(total_tokens) > 0 \
        ORDER BY sum(total_tokens) DESC";

    state
        .clickhouse()
        .query(sql)
        .bind(Identifier(state.config().request_log_table.as_str()))
        .bind(query.start_at.timestamp_millis())
        .bind(query.end_at.timestamp_millis())
        .fetch_all::<UsageUserRow>()
        .await
        .map_err(|source| {
            usage_query_error("user_distribution", UsageScope::AllUsers, query, source)
        })
}

fn usage_query_error(
    query_kind: &'static str,
    scope: UsageScope,
    query: &NormalizedUsageRange,
    source: clickhouse::error::Error,
) -> AppError {
    error!(
        error = %source,
        query_kind,
        usage_scope = scope.as_str(),
        scoped_user_id = ?scope.user_id(),
        start_at = %query.start_at,
        end_at = %query.end_at,
        timezone = %query.timezone,
        "Dashboard 查询 ClickHouse token 用量统计失败"
    );
    AppError::DbQuery {
        message: format!("查询 ClickHouse token 用量统计失败: {source}"),
    }
}

fn build_daily_usage(
    rows: Vec<UsageDailyRow>,
    query: &NormalizedUsageRange,
) -> AppResult<Vec<UsageDailyPoint>> {
    let mut rows_by_date = HashMap::with_capacity(rows.len());
    for row in rows {
        let date = NaiveDate::parse_from_str(&row.usage_date, "%Y-%m-%d").map_err(|source| {
            error!(
                error = %source,
                usage_date = %row.usage_date,
                timezone = %query.timezone,
                "ClickHouse 返回了无法解析的每日用量日期"
            );
            AppError::DbQuery {
                message: format!("ClickHouse 返回无效的每日用量日期 {}", row.usage_date),
            }
        })?;
        if date < query.start_date || date >= query.end_date {
            error!(
                usage_date = %date,
                start_date = %query.start_date,
                end_date = %query.end_date,
                timezone = %query.timezone,
                "ClickHouse 返回了固定年度范围之外的每日用量日期"
            );
            return Err(AppError::DbQuery {
                message: format!("ClickHouse 返回年度范围之外的每日用量日期 {date}"),
            });
        }
        if rows_by_date.insert(date, row).is_some() {
            return Err(AppError::DbQuery {
                message: format!("ClickHouse 返回重复的每日用量日期 {date}"),
            });
        }
    }

    // 响应始终包含连续 365 天。零值补齐放在后端完成，前端只负责把稳定的数据契约映射
    // 为 53×7 网格，避免不同时区或跨年时由浏览器再次推导日期产生偏差。
    (0..USAGE_YEAR_DAYS)
        .map(|offset| {
            let date = query
                .start_date
                .checked_add_days(Days::new(offset))
                .ok_or_else(|| AppError::DbQuery {
                    message: format!("补齐每日用量日期时超出支持范围: offset={offset}"),
                })?;
            let row = rows_by_date.remove(&date);
            Ok(UsageDailyPoint {
                date,
                total_tokens: row
                    .as_ref()
                    .map_or_else(|| "0".to_owned(), |row| row.total_tokens_text.clone()),
                request_count: row
                    .map_or_else(|| "0".to_owned(), |row| row.request_count.to_string()),
            })
        })
        .collect()
}

fn parse_aggregate_for_percentage(value: &str) -> f64 {
    value.parse::<f64>().unwrap_or(0.0).max(0.0)
}

fn token_percentage(value: &str, period_total: f64) -> f64 {
    if period_total <= 0.0 {
        return 0.0;
    }
    let value = parse_aggregate_for_percentage(value);
    ((value / period_total * 10_000.0).round() / 100.0).clamp(0.0, 100.0)
}
