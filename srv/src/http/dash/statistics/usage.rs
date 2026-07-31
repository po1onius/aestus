//! Dashboard token 用量聚合接口。
//!
//! 普通用户只统计自己的余额与请求日志，admin 统计 PostgreSQL 中全部用户的余额及
//! ClickHouse 中全部已归属用户的请求日志。额度属于附属能力，允许极端故障下少量统计
//! 缺失，因此这里不引入账单流水或跨库事务。总量/模型分布与时间趋势使用独立接口，
//! 使趋势密度切换不会重复查询其他统计数据。

use std::collections::HashMap;

use axum::{
    Json, Router,
    extract::{Query, State},
    routing::get,
};
use chrono::{DateTime, Duration, Utc};
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

const MAX_TIME_RANGE_DAYS: i64 = 31;
const MAX_TIMEZONE_BYTES: usize = 64;
const DEFAULT_TIMELINE_POINT_COUNT: u16 = 20;
const DENSE_TIMELINE_POINT_COUNT: u16 = 50;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UsageQuery {
    start_at: Option<DateTime<Utc>>,
    end_at: Option<DateTime<Utc>>,
    timezone: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UsageTimelineQuery {
    start_at: Option<DateTime<Utc>>,
    end_at: Option<DateTime<Utc>>,
    point_count: Option<u16>,
    timezone: Option<String>,
}

#[derive(Debug)]
struct NormalizedUsageRange {
    start_at: DateTime<Utc>,
    end_at: DateTime<Utc>,
    timezone: String,
}

impl NormalizedUsageRange {
    fn duration_millis(&self) -> i64 {
        (self.end_at - self.start_at).num_milliseconds()
    }
}

#[derive(Debug)]
struct NormalizedUsageTimelineQuery {
    range: NormalizedUsageRange,
    point_count: u16,
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
struct UsageTimelineRow {
    bucket_index: u64,
    provider: String,
    model: String,
    total_tokens_text: String,
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
    models: Vec<UsageModelPoint>,
    api_keys: Vec<UsageApiKeyPoint>,
    users: Vec<UsageUserPoint>,
    generated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
struct UsageTimelineResponse {
    scope: &'static str,
    start_at: DateTime<Utc>,
    end_at: DateTime<Utc>,
    point_count: u16,
    timezone: String,
    timeline: Vec<UsageTimelineBucket>,
    generated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
struct UsagePeriodResponse {
    total_tokens: String,
    request_count: String,
}

#[derive(Debug, Serialize)]
struct UsageTimelineBucket {
    index: u16,
    started_at: DateTime<Utc>,
    ended_at: DateTime<Utc>,
    models: Vec<UsageTimelineModelPoint>,
}

#[derive(Debug, Serialize)]
struct UsageTimelineModelPoint {
    provider: String,
    model: String,
    total_tokens: String,
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
    Router::new()
        .route("/", get(get_usage))
        .route("/timeline", get(get_usage_timeline))
}

async fn get_usage(
    State(state): State<AppState>,
    auth::CurrentUser(current_user): auth::CurrentUser,
    Query(query): Query<UsageQuery>,
) -> AppResult<Json<UsageOverviewResponse>> {
    let query = normalize_usage_range(query.start_at, query.end_at, query.timezone)?;
    let scope = UsageScope::from_user(&current_user);

    // PostgreSQL 用户快照、总量、模型分布与角色对应的 Key/用户分布彼此独立，并行查询
    // 可以避免全局日志量或用户量较大时串行延长 Dashboard 首屏等待时间。
    let (users, totals, model_rows, breakdown_rows) = tokio::try_join!(
        load_usage_user_directory(&state, &current_user, scope),
        query_usage_totals(&state, scope, &query),
        query_usage_models(&state, scope, &query),
        query_usage_breakdown(&state, scope, &query),
    )?;

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
        models,
        api_keys,
        users: user_points,
        generated_at: Utc::now(),
    }))
}

async fn get_usage_timeline(
    State(state): State<AppState>,
    auth::CurrentUser(current_user): auth::CurrentUser,
    Query(query): Query<UsageTimelineQuery>,
) -> AppResult<Json<UsageTimelineResponse>> {
    let query = normalize_usage_timeline_query(query)?;
    let scope = UsageScope::from_user(&current_user);
    let timeline_rows = query_usage_timeline(&state, scope, &query).await?;
    let timeline = build_usage_timeline(timeline_rows, &query)?;
    let timeline_model_groups = timeline
        .iter()
        .map(|bucket| bucket.models.len())
        .sum::<usize>();
    let non_empty_timeline_points = timeline
        .iter()
        .filter(|bucket| !bucket.models.is_empty())
        .count();

    info!(
        actor_user_id = %current_user.id,
        usage_scope = scope.as_str(),
        scoped_user_id = ?scope.user_id(),
        start_at = %query.range.start_at,
        end_at = %query.range.end_at,
        point_count = query.point_count,
        timezone = %query.range.timezone,
        timeline_points = timeline.len(),
        non_empty_timeline_points,
        timeline_model_groups,
        "Dashboard token 用量趋势聚合完成"
    );

    Ok(Json(UsageTimelineResponse {
        scope: scope.as_str(),
        start_at: query.range.start_at,
        end_at: query.range.end_at,
        point_count: query.point_count,
        timezone: query.range.timezone,
        timeline,
        generated_at: Utc::now(),
    }))
}

fn normalize_usage_range(
    start_at: Option<DateTime<Utc>>,
    end_at: Option<DateTime<Utc>>,
    timezone: Option<String>,
) -> AppResult<NormalizedUsageRange> {
    let (start_at, end_at) = match (start_at, end_at) {
        (Some(start_at), Some(end_at)) => (start_at, end_at),
        (None, None) => {
            let end_at = Utc::now();
            (end_at - Duration::days(7), end_at)
        }
        _ => {
            return Err(AppError::BadRequest {
                message: "start_at 和 end_at 必须同时传入".to_owned(),
            });
        }
    };
    if start_at >= end_at {
        return Err(AppError::BadRequest {
            message: "start_at 必须早于 end_at".to_owned(),
        });
    }
    if end_at - start_at > Duration::days(MAX_TIME_RANGE_DAYS) {
        return Err(AppError::BadRequest {
            message: format!("用量统计时间跨度不能超过 {MAX_TIME_RANGE_DAYS} 天"),
        });
    }

    let timezone = normalize_timezone(timezone)?;

    Ok(NormalizedUsageRange {
        start_at,
        end_at,
        timezone,
    })
}

fn normalize_usage_timeline_query(
    query: UsageTimelineQuery,
) -> AppResult<NormalizedUsageTimelineQuery> {
    let range = normalize_usage_range(query.start_at, query.end_at, query.timezone)?;
    let point_count = query.point_count.unwrap_or(DEFAULT_TIMELINE_POINT_COUNT);
    if !matches!(
        point_count,
        DEFAULT_TIMELINE_POINT_COUNT | DENSE_TIMELINE_POINT_COUNT
    ) {
        return Err(AppError::BadRequest {
            message: format!(
                "point_count 只支持 {DEFAULT_TIMELINE_POINT_COUNT} 或 {DENSE_TIMELINE_POINT_COUNT}"
            ),
        });
    }
    if range.duration_millis() < i64::from(point_count) {
        return Err(AppError::BadRequest {
            message: format!("统计时间跨度必须至少包含 {point_count} 毫秒"),
        });
    }

    Ok(NormalizedUsageTimelineQuery { range, point_count })
}

fn normalize_timezone(timezone: Option<String>) -> AppResult<String> {
    let timezone = timezone.unwrap_or_else(|| "UTC".to_owned());
    let timezone = timezone.trim();
    let valid = !timezone.is_empty()
        && timezone.len() <= MAX_TIMEZONE_BYTES
        && timezone
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'_' | b'-' | b'+'));
    if !valid {
        return Err(AppError::BadRequest {
            message: "timezone 必须是合法且不超过 64 字节的 IANA 时区名称".to_owned(),
        });
    }
    Ok(timezone.to_owned())
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
        .map_err(|source| usage_query_error("period_totals", scope, query, None, source))
}

async fn query_usage_timeline(
    state: &AppState,
    scope: UsageScope,
    query: &NormalizedUsageTimelineQuery,
) -> AppResult<Vec<UsageTimelineRow>> {
    // 将选定区间从 start_at 起严格等分为 point_count 个桶。使用相对毫秒位置计算桶序号，
    // 不再对齐自然小时或自然日；这样无论选择一天还是三天，X 轴都完整覆盖原区间。
    // provider 必须参与分组，避免不同厂商恰好使用同名模型时被错误合并。
    let current_user_sql = "SELECT \
            toUInt64(intDiv(\
                (toUnixTimestamp64Milli(request_started_at) - ?) * ?, \
                ?\
            )) AS bucket_index, \
            provider, \
            ifNull(model, '未记录') AS model, \
            toString(sum(total_tokens)) AS total_tokens_text \
         FROM ? \
         WHERE user_id = ? \
           AND request_started_at >= fromUnixTimestamp64Milli(?, 'UTC') \
           AND request_started_at < fromUnixTimestamp64Milli(?, 'UTC') \
         GROUP BY bucket_index, provider, model \
         HAVING sum(total_tokens) > 0 \
         ORDER BY bucket_index, sum(total_tokens) DESC";
    let all_users_sql = "SELECT \
            toUInt64(intDiv(\
                (toUnixTimestamp64Milli(request_started_at) - ?) * ?, \
                ?\
            )) AS bucket_index, \
            provider, \
            ifNull(model, '未记录') AS model, \
            toString(sum(total_tokens)) AS total_tokens_text \
         FROM ? \
         WHERE user_id IS NOT NULL \
           AND request_started_at >= fromUnixTimestamp64Milli(?, 'UTC') \
           AND request_started_at < fromUnixTimestamp64Milli(?, 'UTC') \
         GROUP BY bucket_index, provider, model \
         HAVING sum(total_tokens) > 0 \
         ORDER BY bucket_index, sum(total_tokens) DESC";

    let request = state
        .clickhouse()
        .query(match scope {
            UsageScope::CurrentUser(_) => current_user_sql,
            UsageScope::AllUsers => all_users_sql,
        })
        .bind(query.range.start_at.timestamp_millis())
        .bind(i64::from(query.point_count))
        .bind(query.range.duration_millis())
        .bind(Identifier(state.config().request_log_table.as_str()));
    let request = match scope {
        UsageScope::CurrentUser(user_id) => request.bind(user_id),
        UsageScope::AllUsers => request,
    };
    request
        .bind(query.range.start_at.timestamp_millis())
        .bind(query.range.end_at.timestamp_millis())
        .fetch_all::<UsageTimelineRow>()
        .await
        .map_err(|source| {
            usage_query_error(
                "timeline",
                scope,
                &query.range,
                Some(query.point_count),
                source,
            )
        })
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
        .map_err(|source| usage_query_error("model_distribution", scope, query, None, source))
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
                None,
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
            usage_query_error(
                "user_distribution",
                UsageScope::AllUsers,
                query,
                None,
                source,
            )
        })
}

fn usage_query_error(
    query_kind: &'static str,
    scope: UsageScope,
    query: &NormalizedUsageRange,
    point_count: Option<u16>,
    source: clickhouse::error::Error,
) -> AppError {
    error!(
        error = %source,
        query_kind,
        usage_scope = scope.as_str(),
        scoped_user_id = ?scope.user_id(),
        start_at = %query.start_at,
        end_at = %query.end_at,
        point_count,
        timezone = %query.timezone,
        "Dashboard 查询 ClickHouse token 用量统计失败"
    );
    AppError::DbQuery {
        message: format!("查询 ClickHouse token 用量统计失败: {source}"),
    }
}

fn build_usage_timeline(
    rows: Vec<UsageTimelineRow>,
    query: &NormalizedUsageTimelineQuery,
) -> AppResult<Vec<UsageTimelineBucket>> {
    let point_count_i64 = i64::from(query.point_count);
    let start_millis = query.range.start_at.timestamp_millis();
    let duration_millis = query.range.duration_millis();
    let division_rounding = point_count_i64 - 1;

    // 先生成完整桶集合，再填充 ClickHouse 返回的非空模型分组。即使整个区间没有请求，
    // 响应也稳定包含 point_count 个桶，前端无需根据已有数据猜测 X 轴范围。
    let mut timeline = (0..query.point_count)
        .map(|index| {
            // SQL 使用 floor(relative_millis * point_count / duration) 计算桶序号，因此
            // 第 i 个桶的整数毫秒边界应为 ceil(duration * i / point_count)。两者采用同一
            // 取整规则后，即使区间长度不能整除点数，边界毫秒也不会落入相邻桶。
            let started_at_millis = start_millis
                + (duration_millis * i64::from(index) + division_rounding) / point_count_i64;
            let ended_at_millis = start_millis
                + (duration_millis * i64::from(index + 1) + division_rounding) / point_count_i64;
            let started_at =
                DateTime::from_timestamp_millis(started_at_millis).ok_or_else(|| {
                    AppError::DbQuery {
                        message: format!("用量时间桶起点超出支持范围: {started_at_millis}"),
                    }
                })?;
            let ended_at = DateTime::from_timestamp_millis(ended_at_millis).ok_or_else(|| {
                AppError::DbQuery {
                    message: format!("用量时间桶终点超出支持范围: {ended_at_millis}"),
                }
            })?;
            Ok(UsageTimelineBucket {
                index,
                started_at,
                ended_at,
                models: Vec::new(),
            })
        })
        .collect::<AppResult<Vec<_>>>()?;

    for row in rows {
        let index = usize::try_from(row.bucket_index).map_err(|_| AppError::DbQuery {
            message: format!(
                "ClickHouse 用量时间桶序号超出支持范围: {}",
                row.bucket_index
            ),
        })?;
        let Some(bucket) = timeline.get_mut(index) else {
            error!(
                bucket_index = row.bucket_index,
                point_count = query.point_count,
                start_at = %query.range.start_at,
                end_at = %query.range.end_at,
                "ClickHouse 返回了选定时间范围之外的用量桶"
            );
            return Err(AppError::DbQuery {
                message: format!(
                    "ClickHouse 返回无效用量桶序号 {}，最大允许 {}",
                    row.bucket_index,
                    query.point_count - 1
                ),
            });
        };
        bucket.models.push(UsageTimelineModelPoint {
            provider: row.provider,
            model: row.model,
            total_tokens: row.total_tokens_text,
        });
    }

    Ok(timeline)
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
