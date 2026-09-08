//! 请求日志明细查询接口。
//!
//! 本层确定授权范围并校验 HTTP 参数，调用 logs 查询明细；保留期默认 30 天，
//! 由日志模块根据配置校验可查询日期，长期用量由独立日聚合表承担。

use axum::{
    Json, Router,
    extract::{Query, State},
    routing::get,
};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use tracing::warn;
use uuid::Uuid;

use crate::{
    api::console::auth,
    err::{AppError, AppResult},
    state::AppState,
    tenant,
    user::User,
};

use crate::logs::request::{
    RequestLogCursor, RequestLogQuery, RequestLogRecord, normalize_log_date, query_request_log_page,
};

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
    let page = query_request_log_page(
        state.clickhouse(),
        &state.config().request_log_table,
        log_query,
    )
    .await?;

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
