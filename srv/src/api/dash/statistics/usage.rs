//! Dashboard token 用量聚合接口。
//!
//! 普通用户只统计自己的余额与请求日志，租户 owner 统计本租户全部用户，平台管理员统计
//! PostgreSQL 与 ClickHouse 中全部已归属用户的每日预聚合。额度属于附属能力，允许极端故障下少量统计
//! 缺失，因此这里不引入账单流水或跨库事务。用量概览固定统计服务时区下包含今天的
//! 最近 365 个自然日，并一次返回总量、分布与每日明细。

use std::collections::HashMap;

use axum::{Json, Router, extract::State, routing::get};
use chrono::{DateTime, Days, NaiveDate, Utc};
use clickhouse::{Row, sql::Identifier};
use serde::{Deserialize, Serialize};
use tracing::{error, info};
use uuid::Uuid;

use crate::{
    api::dash::auth,
    err::{AppError, AppResult},
    state::AppState,
    user::{self, User},
};

use super::calendar::{current_service_date, local_day_start_utc};

const USAGE_YEAR_DAYS: u64 = 365;

#[derive(Debug)]
struct NormalizedUsageRange {
    start_at: DateTime<Utc>,
    end_at: DateTime<Utc>,
    start_date: NaiveDate,
    end_date: NaiveDate,
    timezone: String,
}

/// 用量查询的数据边界由已认证用户角色唯一决定，不接受调用方通过 query 参数自行扩大。
#[derive(Debug, Clone)]
enum UsageScope {
    CurrentUser(String, Uuid),
    Tenant(String),
    AllUsers,
}

impl UsageScope {
    fn from_user(user: &User) -> Self {
        if user.is_platform_admin() {
            Self::AllUsers
        } else if user.is_tenant_owner() {
            Self::Tenant(user.tenant_id.clone().expect("tenant owner 必须归属租户"))
        } else {
            Self::CurrentUser(
                user.tenant_id.clone().expect("tenant user 必须归属租户"),
                user.id,
            )
        }
    }

    fn as_str(&self) -> &'static str {
        match self {
            Self::CurrentUser(_, _) => "current_user",
            Self::Tenant(_) => "tenant",
            Self::AllUsers => "all_users",
        }
    }

    fn user_id(&self) -> Option<Uuid> {
        match self {
            Self::CurrentUser(_, user_id) => Some(*user_id),
            Self::Tenant(_) | Self::AllUsers => None,
        }
    }

    fn tenant_id(&self) -> Option<&str> {
        match self {
            Self::CurrentUser(tenant_id, _) | Self::Tenant(tenant_id) => Some(tenant_id),
            Self::AllUsers => None,
        }
    }
}

#[derive(Debug)]
struct UsageDirectory {
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
    #[serde(with = "clickhouse::serde::chrono::date")]
    usage_date: NaiveDate,
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
    total_tokens_text: String,
    request_count: u64,
}

#[derive(Debug, Deserialize, Row)]
struct UsageTenantRow {
    tenant_id: String,
    total_tokens_text: String,
    request_count: u64,
}

#[derive(Debug)]
struct UsageBreakdownRows {
    api_keys: Vec<UsageApiKeyRow>,
    users: Vec<UsageUserRow>,
    tenants: Vec<UsageTenantRow>,
}

#[derive(Debug, Serialize)]
struct UsageOverviewResponse {
    scope: &'static str,
    remaining_tokens: String,
    consumed_tokens: String,
    start_at: DateTime<Utc>,
    end_at: DateTime<Utc>,
    timezone: String,
    lifetime: UsageLifetimeResponse,
    daily: Vec<UsageDailyPoint>,
    models: Vec<UsageModelPoint>,
    api_keys: Vec<UsageApiKeyPoint>,
    users: Vec<UsageUserPoint>,
    tenants: Vec<UsageTenantPoint>,
    generated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
struct UsageLifetimeResponse {
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

#[derive(Debug, Serialize)]
struct UsageTenantPoint {
    tenant_id: String,
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
) -> AppResult<Json<UsageOverviewResponse>> {
    let query = normalize_usage_range(state.config().service_timezone)?;
    let scope = UsageScope::from_user(&current_user);

    // 只有 daily 使用固定年度窗口；总量、模型和消费方分布均查询长期日聚合。各查询
    // 彼此独立且数据量已经受控，并行执行可以降低 Dashboard 首屏延迟。
    let (directory, totals, daily_rows, model_rows, breakdown_rows) = tokio::try_join!(
        load_usage_directory(&state, &current_user, &scope),
        query_usage_totals(&state, &scope),
        query_usage_daily(&state, &scope, &query),
        query_usage_models(&state, &scope),
        query_usage_breakdown(&state, &scope),
    )?;
    let daily = build_daily_usage(daily_rows, &query)?;

    let lifetime_total = parse_aggregate_for_percentage(&totals.total_tokens);
    let models = model_rows
        .into_iter()
        .map(|row| UsageModelPoint {
            percentage: token_percentage(&row.total_tokens_text, lifetime_total),
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
            percentage: token_percentage(&row.total_tokens_text, lifetime_total),
            name: row.api_key_name_text,
            total_tokens: row.total_tokens_text,
            request_count: row.request_count.to_string(),
        })
        .collect::<Vec<_>>();
    let user_points = breakdown_rows
        .users
        .into_iter()
        .map(|row| UsageUserPoint {
            percentage: token_percentage(&row.total_tokens_text, lifetime_total),
            username: directory
                .usernames_by_id
                .get(&row.user_id)
                .cloned()
                .unwrap_or_else(|| format!("未知用户 ({})", row.user_id)),
            user_id: row.user_id,
            total_tokens: row.total_tokens_text,
            request_count: row.request_count.to_string(),
        })
        .collect::<Vec<_>>();
    let tenant_points = breakdown_rows
        .tenants
        .into_iter()
        .map(|row| UsageTenantPoint {
            percentage: token_percentage(&row.total_tokens_text, lifetime_total),
            tenant_id: row.tenant_id,
            total_tokens: row.total_tokens_text,
            request_count: row.request_count.to_string(),
        })
        .collect::<Vec<_>>();

    info!(
        actor_user_id = %current_user.id,
        usage_scope = scope.as_str(),
        scoped_user_id = ?scope.user_id(),
        scoped_tenant_id = ?scope.tenant_id(),
        annual_start_at = %query.start_at,
        annual_end_at = %query.end_at,
        timezone = %query.timezone,
        lifetime_total_tokens = %totals.total_tokens,
        lifetime_request_count = totals.request_count,
        daily_points = daily.len(),
        active_days = daily.iter().filter(|point| point.total_tokens != "0").count(),
        model_groups = models.len(),
        api_key_groups = api_keys.len(),
        user_groups = user_points.len(),
        tenant_groups = tenant_points.len(),
        "Dashboard token 用量概览聚合完成"
    );

    Ok(Json(UsageOverviewResponse {
        scope: scope.as_str(),
        remaining_tokens: directory.remaining_tokens,
        consumed_tokens: directory.consumed_tokens,
        start_at: query.start_at,
        end_at: query.end_at,
        timezone: query.timezone,
        lifetime: UsageLifetimeResponse {
            total_tokens: totals.total_tokens,
            request_count: totals.request_count.to_string(),
        },
        daily,
        models,
        api_keys,
        users: user_points,
        tenants: tenant_points,
        generated_at: Utc::now(),
    }))
}

fn normalize_usage_range(timezone: chrono_tz::Tz) -> AppResult<NormalizedUsageRange> {
    let today = current_service_date(timezone);
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
        timezone: timezone.name().to_owned(),
    })
}

async fn load_usage_directory(
    state: &AppState,
    current_user: &User,
    scope: &UsageScope,
) -> AppResult<UsageDirectory> {
    if matches!(scope, UsageScope::CurrentUser(_, _)) {
        return Ok(UsageDirectory {
            remaining_tokens: current_user.quota.to_string(),
            consumed_tokens: current_user.consumed_tokens.to_string(),
            usernames_by_id: HashMap::from([(current_user.id, current_user.username.clone())]),
        });
    }

    let mut conn = state.db_conn().await?;
    let snapshots = user::list_usage_snapshots(&mut conn, scope.tenant_id()).await?;
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
    Ok(UsageDirectory {
        remaining_tokens: remaining_tokens.to_string(),
        consumed_tokens: consumed_tokens.to_string(),
        usernames_by_id,
    })
}

async fn query_usage_totals(state: &AppState, scope: &UsageScope) -> AppResult<UsageTotalsRow> {
    let current_user_sql = "SELECT \
        toString(sum(total_tokens)) AS total_tokens, \
        sum(request_count) AS request_count \
        FROM ? \
        WHERE tenant_id = ? AND user_id = ?";
    let tenant_sql = "SELECT \
        toString(sum(total_tokens)) AS total_tokens, \
        sum(request_count) AS request_count \
        FROM ? \
        WHERE tenant_id = ?";
    let all_users_sql = "SELECT \
        toString(sum(total_tokens)) AS total_tokens, \
        sum(request_count) AS request_count \
        FROM ?";

    let request = state
        .clickhouse()
        .query(match scope {
            UsageScope::CurrentUser(_, _) => current_user_sql,
            UsageScope::Tenant(_) => tenant_sql,
            UsageScope::AllUsers => all_users_sql,
        })
        .bind(Identifier(
            state.config().request_usage_daily_table.as_str(),
        ));
    let request = match scope {
        UsageScope::CurrentUser(tenant_id, user_id) => request.bind(tenant_id).bind(user_id),
        UsageScope::Tenant(tenant_id) => request.bind(tenant_id),
        UsageScope::AllUsers => request,
    };
    request
        .fetch_one::<UsageTotalsRow>()
        .await
        .map_err(|source| usage_query_error("lifetime_totals", scope, source))
}

async fn query_usage_daily(
    state: &AppState,
    scope: &UsageScope,
    query: &NormalizedUsageRange,
) -> AppResult<Vec<UsageDailyRow>> {
    // usage_date 已由 writer 按服务固定时区计算并持久化，这里只扫描 365 天的
    // 预聚合行，不再在查询时重新解释时区或请求级时间戳。
    let current_user_sql = "SELECT \
            usage_date, \
            toString(sum(total_tokens)) AS total_tokens_text, \
            sum(request_count) AS request_count \
         FROM ? \
         WHERE tenant_id = ? AND user_id = ? \
           AND usage_date >= toDate(?) \
           AND usage_date < toDate(?) \
         GROUP BY usage_date \
         ORDER BY usage_date";
    let tenant_sql = "SELECT \
            usage_date, \
            toString(sum(total_tokens)) AS total_tokens_text, \
            sum(request_count) AS request_count \
         FROM ? \
         WHERE tenant_id = ? \
           AND usage_date >= toDate(?) \
           AND usage_date < toDate(?) \
         GROUP BY usage_date \
         ORDER BY usage_date";
    let all_users_sql = "SELECT \
            usage_date, \
            toString(sum(total_tokens)) AS total_tokens_text, \
            sum(request_count) AS request_count \
         FROM ? \
         WHERE usage_date >= toDate(?) \
           AND usage_date < toDate(?) \
         GROUP BY usage_date \
         ORDER BY usage_date";

    let request = state
        .clickhouse()
        .query(match scope {
            UsageScope::CurrentUser(_, _) => current_user_sql,
            UsageScope::Tenant(_) => tenant_sql,
            UsageScope::AllUsers => all_users_sql,
        })
        .bind(Identifier(
            state.config().request_usage_daily_table.as_str(),
        ));
    let request = match scope {
        UsageScope::CurrentUser(tenant_id, user_id) => request.bind(tenant_id).bind(user_id),
        UsageScope::Tenant(tenant_id) => request.bind(tenant_id),
        UsageScope::AllUsers => request,
    };
    request
        .bind(query.start_date.to_string())
        .bind(query.end_date.to_string())
        .fetch_all::<UsageDailyRow>()
        .await
        .map_err(|source| usage_daily_query_error(scope, query, source))
}

async fn query_usage_models(state: &AppState, scope: &UsageScope) -> AppResult<Vec<UsageModelRow>> {
    // ClickHouse 的 SELECT 别名在 HAVING/ORDER BY 中可见。如果字符串结果也命名为
    // total_tokens，后面的 sum(total_tokens) 会被解析为对 String 别名再次聚合。内部使用
    // 独立别名，确保聚合表达式始终引用请求日志的 Int64 原始列。
    let current_user_sql = "SELECT \
        provider, \
        model, \
        toString(sum(total_tokens)) AS total_tokens_text, \
        sum(request_count) AS request_count \
        FROM ? \
        WHERE tenant_id = ? AND user_id = ? \
        GROUP BY provider, model \
        HAVING sum(total_tokens) > 0 \
        ORDER BY sum(total_tokens) DESC";
    let tenant_sql = "SELECT \
        provider, \
        model, \
        toString(sum(total_tokens)) AS total_tokens_text, \
        sum(request_count) AS request_count \
        FROM ? \
        WHERE tenant_id = ? \
        GROUP BY provider, model \
        HAVING sum(total_tokens) > 0 \
        ORDER BY sum(total_tokens) DESC";
    let all_users_sql = "SELECT \
        provider, \
        model, \
        toString(sum(total_tokens)) AS total_tokens_text, \
        sum(request_count) AS request_count \
        FROM ? \
        GROUP BY provider, model \
        HAVING sum(total_tokens) > 0 \
        ORDER BY sum(total_tokens) DESC";

    let request = state
        .clickhouse()
        .query(match scope {
            UsageScope::CurrentUser(_, _) => current_user_sql,
            UsageScope::Tenant(_) => tenant_sql,
            UsageScope::AllUsers => all_users_sql,
        })
        .bind(Identifier(
            state.config().request_usage_daily_table.as_str(),
        ));
    let request = match scope {
        UsageScope::CurrentUser(tenant_id, user_id) => request.bind(tenant_id).bind(user_id),
        UsageScope::Tenant(tenant_id) => request.bind(tenant_id),
        UsageScope::AllUsers => request,
    };
    request
        .fetch_all::<UsageModelRow>()
        .await
        .map_err(|source| usage_query_error("lifetime_model_distribution", scope, source))
}

async fn query_usage_breakdown(
    state: &AppState,
    scope: &UsageScope,
) -> AppResult<UsageBreakdownRows> {
    match scope {
        UsageScope::CurrentUser(tenant_id, user_id) => Ok(UsageBreakdownRows {
            api_keys: query_usage_api_keys(state, tenant_id, *user_id).await?,
            users: Vec::new(),
            tenants: Vec::new(),
        }),
        UsageScope::Tenant(tenant_id) => Ok(UsageBreakdownRows {
            api_keys: Vec::new(),
            users: query_usage_users(state, tenant_id).await?,
            tenants: Vec::new(),
        }),
        UsageScope::AllUsers => Ok(UsageBreakdownRows {
            api_keys: Vec::new(),
            users: Vec::new(),
            tenants: query_usage_tenants(state).await?,
        }),
    }
}

async fn query_usage_api_keys(
    state: &AppState,
    tenant_id: &str,
    user_id: Uuid,
) -> AppResult<Vec<UsageApiKeyRow>> {
    // API Key 名称在单个用户内唯一且不支持改名，普通用户视图可以直接按名称聚合。
    let sql = "SELECT \
        api_key_name AS api_key_name_text, \
        toString(sum(total_tokens)) AS total_tokens_text, \
        sum(request_count) AS request_count \
        FROM ? \
        WHERE tenant_id = ? AND user_id = ? \
        GROUP BY api_key_name_text \
        HAVING sum(total_tokens) > 0 \
        ORDER BY sum(total_tokens) DESC";

    state
        .clickhouse()
        .query(sql)
        .bind(Identifier(
            state.config().request_usage_daily_table.as_str(),
        ))
        .bind(tenant_id)
        .bind(user_id)
        .fetch_all::<UsageApiKeyRow>()
        .await
        .map_err(|source| {
            usage_query_error(
                "lifetime_api_key_distribution",
                &UsageScope::CurrentUser(tenant_id.to_owned(), user_id),
                source,
            )
        })
}

async fn query_usage_tenants(state: &AppState) -> AppResult<Vec<UsageTenantRow>> {
    let sql = "SELECT \
        tenant_id, \
        toString(sum(total_tokens)) AS total_tokens_text, \
        sum(request_count) AS request_count \
        FROM ? \
        GROUP BY tenant_id \
        HAVING sum(total_tokens) > 0 \
        ORDER BY sum(total_tokens) DESC";

    state
        .clickhouse()
        .query(sql)
        .bind(Identifier(
            state.config().request_usage_daily_table.as_str(),
        ))
        .fetch_all::<UsageTenantRow>()
        .await
        .map_err(|source| {
            usage_query_error(
                "lifetime_tenant_distribution",
                &UsageScope::AllUsers,
                source,
            )
        })
}

async fn query_usage_users(state: &AppState, tenant_id: &str) -> AppResult<Vec<UsageUserRow>> {
    let sql = "SELECT \
        user_id, \
        toString(sum(total_tokens)) AS total_tokens_text, \
        sum(request_count) AS request_count \
        FROM ? \
        WHERE tenant_id = ? \
        GROUP BY user_id \
        HAVING sum(total_tokens) > 0 \
        ORDER BY sum(total_tokens) DESC";

    state
        .clickhouse()
        .query(sql)
        .bind(Identifier(
            state.config().request_usage_daily_table.as_str(),
        ))
        .bind(tenant_id)
        .fetch_all::<UsageUserRow>()
        .await
        .map_err(|source| {
            usage_query_error(
                "lifetime_user_distribution",
                &UsageScope::Tenant(tenant_id.to_owned()),
                source,
            )
        })
}

fn usage_query_error(
    query_kind: &'static str,
    scope: &UsageScope,
    source: clickhouse::error::Error,
) -> AppError {
    error!(
        error = %source,
        query_kind,
        usage_scope = scope.as_str(),
        scoped_user_id = ?scope.user_id(),
        scoped_tenant_id = ?scope.tenant_id(),
        "Dashboard 查询 ClickHouse 全历史 token 用量统计失败"
    );
    AppError::DbQuery {
        message: format!("查询 ClickHouse token 用量统计失败: {source}"),
    }
}

fn usage_daily_query_error(
    scope: &UsageScope,
    query: &NormalizedUsageRange,
    source: clickhouse::error::Error,
) -> AppError {
    error!(
        error = %source,
        query_kind = "annual_daily",
        usage_scope = scope.as_str(),
        scoped_user_id = ?scope.user_id(),
        start_at = %query.start_at,
        end_at = %query.end_at,
        timezone = %query.timezone,
        "Dashboard 查询 ClickHouse 年度每日 token 用量统计失败"
    );
    AppError::DbQuery {
        message: format!("查询 ClickHouse 年度每日 token 用量统计失败: {source}"),
    }
}

fn build_daily_usage(
    rows: Vec<UsageDailyRow>,
    query: &NormalizedUsageRange,
) -> AppResult<Vec<UsageDailyPoint>> {
    let mut rows_by_date = HashMap::with_capacity(rows.len());
    for row in rows {
        let date = row.usage_date;
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

fn token_percentage(value: &str, lifetime_total: f64) -> f64 {
    if lifetime_total <= 0.0 {
        return 0.0;
    }
    let value = parse_aggregate_for_percentage(value);
    ((value / lifetime_total * 10_000.0).round() / 100.0).clamp(0.0, 100.0)
}
