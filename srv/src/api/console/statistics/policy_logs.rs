//! 租户 owner 的 GPT 策略日志查询，按服务时区的自然日和稳定游标分页。

use axum::{
    Json, Router,
    extract::{Query, State},
    routing::get,
};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use tracing::info;
use uuid::Uuid;

use crate::{
    api::console::auth,
    err::{AppError, AppResult},
    logs::policy::{PolicyLogCursor, PolicyLogQuery, PolicyLogRecord, query_policy_log_page},
    state::AppState,
};

use crate::logs::calendar::{current_service_date, local_day_range_utc};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ListPolicyLogsQuery {
    limit: Option<usize>,
    date: Option<NaiveDate>,
    before_occurred_at: Option<DateTime<Utc>>,
    before_id: Option<Uuid>,
}

#[derive(Serialize)]
struct ListPolicyLogsResponse {
    date: NaiveDate,
    timezone: String,
    items: Vec<PolicyLogRecord>,
    next_cursor: Option<PolicyLogCursor>,
}

pub(super) fn router() -> Router<AppState> {
    Router::new().route("/", get(list_policy_logs))
}

async fn list_policy_logs(
    State(state): State<AppState>,
    auth::AdminUser(owner): auth::AdminUser,
    Query(params): Query<ListPolicyLogsQuery>,
) -> AppResult<Json<ListPolicyLogsResponse>> {
    // 租户范围只取鉴权结果，接口不接受客户端指定租户。
    let tenant_id = owner.tenant_id.as_deref().ok_or(AppError::Forbidden)?;
    let limit = params.limit.unwrap_or(100);
    if !(1..=500).contains(&limit) {
        return Err(AppError::BadRequest {
            message: "limit 必须在 1 到 500 之间".to_owned(),
        });
    }
    let timezone = state.config().service_timezone;
    let today = current_service_date(timezone);
    let date = params.date.unwrap_or(today);
    if date > today {
        return Err(AppError::BadRequest {
            message: "不能查询未来日期的 Policy 日志".to_owned(),
        });
    }
    let (start_at, end_at) = local_day_range_utc(timezone, date)?;
    let cursor = match (params.before_occurred_at, params.before_id) {
        (None, None) => None,
        (Some(at), Some(id)) if at >= start_at && at < end_at => Some((at, id)),
        (Some(_), Some(_)) => {
            return Err(AppError::BadRequest {
                message: "Policy 日志游标必须位于查询日期内".to_owned(),
            });
        }
        _ => {
            return Err(AppError::BadRequest {
                message: "before_occurred_at 和 before_id 必须同时提供".to_owned(),
            });
        }
    };

    let mut conn = state.db_conn().await?;
    let page = query_policy_log_page(
        &mut conn,
        PolicyLogQuery {
            tenant_id,
            start_at,
            end_at,
            limit,
            cursor,
        },
    )
    .await?;
    info!(
        owner_id = %owner.id, tenant_id, %date, %timezone, limit, ?cursor,
        count = page.items.len(), has_more = page.next_cursor.is_some(), "Policy 日志查询完成"
    );
    Ok(Json(ListPolicyLogsResponse {
        date,
        timezone: timezone.to_string(),
        items: page.items,
        next_cursor: page.next_cursor,
    }))
}
