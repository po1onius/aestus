//! 请求日志明细查询接口。
//!
//! 查询按服务固定时区的单个自然日使用 ClickHouse 时间裁剪和 keyset 分页；明细只保留
//! 30 天，长期用量由独立日聚合表承担。

use std::num::NonZeroU32;

use axum::{
    Json, Router,
    extract::{Query, State},
    routing::get,
};
use chrono::{DateTime, Days, NaiveDate, Utc};
use clickhouse::{Row, sql::Identifier};
use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::{
    api::dash::auth,
    err::{AppError, AppResult},
    state::AppState,
    tenant,
    user::User,
};

use super::calendar::{current_service_date, local_day_range_utc};

const DEFAULT_LIMIT: usize = 100;
const MAX_LIMIT: usize = 500;
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ListRequestLogsQuery {
    limit: Option<usize>,
    /// 服务固定时区下的自然日；省略时查询当天。
    date: Option<NaiveDate>,
    /// 仅平台管理员可以指定；省略时平台管理员查询全平台日志。
    tenant_id: Option<String>,
    before_started_at: Option<DateTime<Utc>>,
    before_request_id: Option<Uuid>,
    non_success_only: Option<bool>,
}

#[derive(Debug, Serialize)]
struct ListRequestLogsResponse {
    date: NaiveDate,
    timezone: String,
    items: Vec<RequestLogRecord>,
    next_cursor: Option<RequestLogCursor>,
}

/// 请求日志查询结果使用独立的 ClickHouse 读取行，不依赖 worker 的写入 DTO。
///
/// 后续统计接口可以为各自的 SELECT 单独定义更小的聚合结果类型，避免为了查询少量指标
/// 解码完整请求日志行。
#[derive(Debug, Deserialize, Row)]
struct RequestLogQueryRow {
    #[serde(with = "clickhouse::serde::uuid")]
    request_id: Uuid,
    #[serde(with = "clickhouse::serde::uuid::option")]
    resource_id: Option<Uuid>,
    provider: String,
    route: String,
    api_key_name: Option<String>,
    tenant_id: Option<String>,
    #[serde(with = "clickhouse::serde::uuid::option")]
    user_id: Option<Uuid>,
    username: Option<String>,
    #[serde(with = "clickhouse::serde::uuid::option")]
    provider_group_id: Option<Uuid>,
    provider_group_name: Option<String>,
    model: Option<String>,
    reasoning: Option<String>,
    service_tier: Option<String>,
    fast_mode: Option<bool>,
    is_compaction: Option<bool>,
    #[serde(with = "clickhouse::serde::chrono::datetime64::millis")]
    request_started_at: DateTime<Utc>,
    #[serde(with = "clickhouse::serde::chrono::datetime64::millis::option")]
    response_started_at: Option<DateTime<Utc>>,
    #[serde(with = "clickhouse::serde::chrono::datetime64::millis::option")]
    response_finished_at: Option<DateTime<Utc>>,
    input_tokens: i64,
    cached_input_tokens: i64,
    output_tokens: i64,
    reasoning_output_tokens: i64,
    total_tokens: i64,
    status: String,
    extra: String,
}

#[derive(Debug, Serialize)]
struct RequestLogRecord {
    request_id: Uuid,
    resource_id: Option<Uuid>,
    provider: String,
    route: String,
    api_key_name: Option<String>,
    tenant_id: Option<String>,
    user_id: Option<Uuid>,
    username: Option<String>,
    provider_group_id: Option<Uuid>,
    provider_group_name: Option<String>,
    model: Option<String>,
    reasoning: Option<String>,
    service_tier: Option<String>,
    fast_mode: Option<bool>,
    is_compaction: Option<bool>,
    request_started_at: DateTime<Utc>,
    response_started_at: Option<DateTime<Utc>>,
    response_finished_at: Option<DateTime<Utc>>,
    duration_ms: Option<i64>,
    input_tokens: i64,
    cached_input_tokens: i64,
    output_tokens: i64,
    reasoning_output_tokens: i64,
    total_tokens: i64,
    status: String,
    extra: serde_json::Value,
}

impl From<RequestLogQueryRow> for RequestLogRecord {
    fn from(row: RequestLogQueryRow) -> Self {
        let duration_ms = row
            .response_finished_at
            .map(|finished_at| (finished_at - row.request_started_at).num_milliseconds());
        let extra = parse_extra_json(row.request_id, &row.extra);

        Self {
            request_id: row.request_id,
            resource_id: row.resource_id,
            provider: row.provider,
            route: row.route,
            api_key_name: row.api_key_name,
            tenant_id: row.tenant_id,
            user_id: row.user_id,
            username: row.username,
            provider_group_id: row.provider_group_id,
            provider_group_name: row.provider_group_name,
            model: row.model,
            reasoning: row.reasoning,
            service_tier: row.service_tier,
            fast_mode: row.fast_mode,
            is_compaction: row.is_compaction,
            request_started_at: row.request_started_at,
            response_started_at: row.response_started_at,
            response_finished_at: row.response_finished_at,
            duration_ms,
            input_tokens: row.input_tokens,
            cached_input_tokens: row.cached_input_tokens,
            output_tokens: row.output_tokens,
            reasoning_output_tokens: row.reasoning_output_tokens,
            total_tokens: row.total_tokens,
            status: row.status,
            extra,
        }
    }
}

#[derive(Debug)]
struct RequestLogQuery {
    limit: usize,
    tenant_id: Option<String>,
    user_id: Option<Uuid>,
    start_at: DateTime<Utc>,
    end_at: DateTime<Utc>,
    before_started_at: Option<DateTime<Utc>>,
    before_request_id: Option<Uuid>,
    non_success_only: bool,
}

struct RequestLogDateRange {
    date: NaiveDate,
    start_at: DateTime<Utc>,
    end_at: DateTime<Utc>,
}

struct RequestLogPage {
    items: Vec<RequestLogRecord>,
    next_cursor: Option<RequestLogCursor>,
}

#[derive(Debug, Serialize)]
struct RequestLogCursor {
    before_started_at: DateTime<Utc>,
    before_request_id: Uuid,
}

pub fn router() -> Router<AppState> {
    Router::new().route("/", get(list_request_logs))
}

async fn list_request_logs(
    State(state): State<AppState>,
    auth::CurrentUser(current_user): auth::CurrentUser,
    Query(query): Query<ListRequestLogsQuery>,
) -> AppResult<Json<ListRequestLogsResponse>> {
    let ListRequestLogsQuery {
        limit,
        date,
        tenant_id: requested_tenant_id,
        before_started_at,
        before_request_id,
        non_success_only,
    } = query;
    let limit = normalize_limit(limit)?;
    let timezone = state.config().service_timezone;
    let retention_days = state.config().request_log_retention_days;
    let date_range = normalize_log_date(date, timezone, retention_days)?;
    let (before_started_at, before_request_id) = normalize_cursor(
        before_started_at,
        before_request_id,
        date_range.start_at,
        date_range.end_at,
    )?;
    let tenant_id = resolve_tenant_scope(&state, &current_user, requested_tenant_id).await?;
    let log_query = RequestLogQuery {
        limit,
        tenant_id,
        user_id: (!current_user.is_platform_admin() && !current_user.is_tenant_owner())
            .then_some(current_user.id),
        start_at: date_range.start_at,
        end_at: date_range.end_at,
        before_started_at,
        before_request_id,
        non_success_only: non_success_only.unwrap_or(false),
    };
    let page = query_request_log_page(&state, log_query).await?;

    Ok(Json(ListRequestLogsResponse {
        date: date_range.date,
        timezone: timezone.name().to_owned(),
        items: page.items,
        next_cursor: page.next_cursor,
    }))
}

async fn resolve_tenant_scope(
    state: &AppState,
    current_user: &User,
    requested_tenant_id: Option<String>,
) -> AppResult<Option<String>> {
    if !current_user.is_platform_admin() {
        if let Some(requested_tenant_id) = requested_tenant_id {
            warn!(
                user_id = %current_user.id,
                role = %current_user.role,
                own_tenant_id = ?current_user.tenant_id,
                requested_tenant_id,
                "非平台管理员尝试指定请求日志租户筛选"
            );
            return Err(AppError::Forbidden);
        }
        return Ok(current_user.tenant_id.clone());
    }

    let Some(requested_tenant_id) = requested_tenant_id else {
        return Ok(None);
    };
    let tenant_id = tenant::normalize_name(requested_tenant_id)?;
    let mut conn = state.db_conn().await?;
    if tenant::find_by_id(&mut conn, &tenant_id).await?.is_none() {
        warn!(
            platform_admin_id = %current_user.id,
            tenant_id,
            "平台管理员查询请求日志时指定了不存在的租户"
        );
        return Err(AppError::BadRequest {
            message: format!("租户不存在: {tenant_id}"),
        });
    }

    Ok(Some(tenant_id))
}

fn normalize_limit(limit: Option<usize>) -> AppResult<usize> {
    let limit = limit.unwrap_or(DEFAULT_LIMIT);
    if limit == 0 || limit > MAX_LIMIT {
        return Err(AppError::BadRequest {
            message: format!("limit 必须在 1 到 {MAX_LIMIT} 之间"),
        });
    }

    Ok(limit)
}

fn normalize_log_date(
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

fn normalize_cursor(
    before_started_at: Option<DateTime<Utc>>,
    before_request_id: Option<Uuid>,
    start_at: DateTime<Utc>,
    end_at: DateTime<Utc>,
) -> AppResult<(Option<DateTime<Utc>>, Option<Uuid>)> {
    match (before_started_at, before_request_id) {
        (Some(before_started_at), Some(before_request_id)) => {
            if before_started_at < start_at || before_started_at >= end_at {
                return Err(AppError::BadRequest {
                    message: "请求日志分页光标不属于当前查询日期".to_owned(),
                });
            }
            Ok((Some(before_started_at), Some(before_request_id)))
        }
        (None, None) => Ok((None, None)),
        _ => Err(AppError::BadRequest {
            message: "before_started_at 和 before_request_id 必须同时传入".to_owned(),
        }),
    }
}

async fn query_request_log_page(
    state: &AppState,
    query: RequestLogQuery,
) -> AppResult<RequestLogPage> {
    let fetch_limit = u64::try_from(query.limit.saturating_add(1)).unwrap_or(u64::MAX);
    let sql = request_log_page_sql(&query);
    let table = state.config().request_log_table.as_str();
    let mut clickhouse_query = state
        .clickhouse()
        .query(&sql)
        .bind(Identifier(table))
        .bind(datetime_millis(query.start_at))
        .bind(datetime_millis(query.end_at));

    if let Some(tenant_id) = query.tenant_id.as_deref() {
        clickhouse_query = clickhouse_query.bind(tenant_id);
    }
    if let Some(user_id) = query.user_id {
        clickhouse_query = clickhouse_query.bind(user_id);
    }
    if let (Some(before_started_at), Some(before_request_id)) =
        (query.before_started_at, query.before_request_id)
    {
        clickhouse_query = clickhouse_query
            .bind(datetime_millis(before_started_at))
            .bind(datetime_millis(before_started_at))
            .bind(before_request_id);
    }
    clickhouse_query = clickhouse_query.bind(fetch_limit);

    let rows = match clickhouse_query.fetch_all::<RequestLogQueryRow>().await {
        Ok(rows) => rows,
        Err(query_error) => {
            error!(
                error = %query_error,
                clickhouse_table = table,
                tenant_id = ?query.tenant_id,
                user_id = query.user_id.map(|id| id.to_string()).unwrap_or_else(|| "<all>".to_owned()),
                limit = query.limit,
                start_at = %query.start_at,
                end_at = %query.end_at,
                non_success_only = query.non_success_only,
                "Dashboard 查询 ClickHouse 请求日志分页失败"
            );
            return Err(AppError::DbQuery {
                message: format!("查询 ClickHouse 请求日志失败: {query_error}"),
            });
        }
    };

    let has_next = rows.len() > query.limit;
    let mut records = rows
        .into_iter()
        .take(query.limit)
        .map(RequestLogRecord::from)
        .collect::<Vec<_>>();
    let next_cursor = if has_next {
        records.last().map(|record| RequestLogCursor {
            before_started_at: record.request_started_at,
            before_request_id: record.request_id,
        })
    } else {
        None
    };

    info!(
        clickhouse_table = table,
        tenant_id = ?query.tenant_id,
        user_id = query.user_id.map(|id| id.to_string()).unwrap_or_else(|| "<all>".to_owned()),
        limit = query.limit,
        returned_rows = records.len(),
        has_next = next_cursor.is_some(),
        start_at = %query.start_at,
        end_at = %query.end_at,
        non_success_only = query.non_success_only,
        "Dashboard 请求日志分页查询完成"
    );

    records.shrink_to_fit();
    Ok(RequestLogPage {
        items: records,
        next_cursor,
    })
}

fn parse_extra_json(request_id: Uuid, extra: &str) -> serde_json::Value {
    match serde_json::from_str(extra) {
        Ok(value) => value,
        Err(parse_error) => {
            warn!(
                request_id = %request_id,
                error = %parse_error,
                "请求日志 extra 不是合法 JSON，Dashboard 返回原始字符串"
            );
            serde_json::json!({ "raw_extra": extra })
        }
    }
}

fn datetime_millis(value: DateTime<Utc>) -> i64 {
    value.timestamp_millis()
}

fn request_log_page_sql(query: &RequestLogQuery) -> String {
    let mut conditions = vec![
        "request_started_at >= fromUnixTimestamp64Milli(?, 'UTC')".to_owned(),
        "request_started_at < fromUnixTimestamp64Milli(?, 'UTC')".to_owned(),
    ];

    if query.user_id.is_some() {
        conditions.push("user_id = ?".to_owned());
    }
    if query.tenant_id.is_some() {
        conditions.insert(2, "tenant_id = ?".to_owned());
    }
    if query.non_success_only {
        conditions.push("status IN ('abnormal', 'failed')".to_owned());
    }
    if query.before_started_at.is_some() && query.before_request_id.is_some() {
        conditions.push(
            "(request_started_at < fromUnixTimestamp64Milli(?, 'UTC') OR (request_started_at = fromUnixTimestamp64Milli(?, 'UTC') AND request_id < ?))".to_owned(),
        );
    }

    format!(
        "SELECT ?fields FROM ? WHERE {} ORDER BY request_started_at DESC, request_id DESC LIMIT ?",
        conditions.join(" AND ")
    )
}
