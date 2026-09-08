//! 请求日志明细查询；调用方必须先确定授权范围并校验查询参数。

use chrono::{DateTime, Utc};
use clickhouse::{Row, sql::Identifier};
use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::err::{AppError, AppResult};

/// 请求日志查询结果使用独立的 ClickHouse 读取行，不依赖 writer 的写入 DTO。
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
pub(crate) struct RequestLogRecord {
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
pub(crate) struct RequestLogQuery {
    pub limit: usize,
    pub tenant_id: Option<String>,
    pub user_id: Option<Uuid>,
    pub start_at: DateTime<Utc>,
    pub end_at: DateTime<Utc>,
    pub before_started_at: Option<DateTime<Utc>>,
    pub before_request_id: Option<Uuid>,
    pub non_success_only: bool,
}

pub(crate) struct RequestLogPage {
    pub items: Vec<RequestLogRecord>,
    pub next_cursor: Option<RequestLogCursor>,
}

#[derive(Debug, Serialize)]
pub(crate) struct RequestLogCursor {
    before_started_at: DateTime<Utc>,
    before_request_id: Uuid,
}

pub(crate) async fn query_request_log_page(
    client: &clickhouse::Client,
    table: &str,
    query: RequestLogQuery,
) -> AppResult<RequestLogPage> {
    let fetch_limit = u64::try_from(query.limit.saturating_add(1)).unwrap_or(u64::MAX);
    let sql = request_log_page_sql(&query);
    let mut clickhouse_query = client
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
