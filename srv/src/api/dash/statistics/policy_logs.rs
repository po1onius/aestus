//! 租户 owner 的 GPT 策略日志查询，按服务时区的自然日和稳定游标分页。

use axum::{
    Json, Router,
    extract::{Query, State},
    routing::get,
};
use chrono::{DateTime, NaiveDate, Utc};
use diesel::prelude::*;
use diesel_async::RunQueryDsl;
use serde::{Deserialize, Serialize};
use tracing::{error, info};
use uuid::Uuid;

use crate::{
    api::dash::auth,
    err::{AppError, AppResult},
    request::policy_log::gpt_policy_violation_logs,
    state::AppState,
};

use super::calendar::{current_service_date, local_day_range_utc};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ListPolicyLogsQuery {
    limit: Option<usize>,
    date: Option<NaiveDate>,
    before_occurred_at: Option<DateTime<Utc>>,
    before_id: Option<Uuid>,
}

#[derive(Queryable, Selectable, Serialize)]
#[diesel(table_name = gpt_policy_violation_logs)]
#[diesel(check_for_backend(diesel::pg::Pg))]
struct PolicyLogRecord {
    id: Uuid,
    username: String,
    account_email: Option<String>,
    occurred_at: DateTime<Utc>,
    error_code: String,
}

#[derive(Serialize)]
struct PolicyLogCursor {
    before_occurred_at: DateTime<Utc>,
    before_id: Uuid,
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
    use gpt_policy_violation_logs::dsl;

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

    let mut query = dsl::gpt_policy_violation_logs
        .filter(dsl::tenant_id.eq(tenant_id))
        .filter(dsl::occurred_at.ge(start_at))
        .filter(dsl::occurred_at.lt(end_at))
        .into_boxed();
    if let Some((at, id)) = cursor {
        query = query.filter(
            dsl::occurred_at
                .lt(at)
                .or(dsl::occurred_at.eq(at).and(dsl::id.lt(id))),
        );
    }
    let mut conn = state.db_conn().await?;
    let mut items = query
        .order((dsl::occurred_at.desc(), dsl::id.desc()))
        .limit((limit + 1) as i64)
        .select(PolicyLogRecord::as_select())
        .load::<PolicyLogRecord>(&mut conn)
        .await
        .map_err(|source| {
            error!(owner_id = %owner.id, tenant_id, %date, ?cursor, %source, "查询 Policy 日志失败");
            AppError::DbQuery {
                message: format!("查询 Policy 日志失败: {source}"),
            }
        })?;
    let has_more = items.len() > limit;
    items.truncate(limit);
    let next_cursor = if has_more {
        items.last().map(|row| PolicyLogCursor {
            before_occurred_at: row.occurred_at,
            before_id: row.id,
        })
    } else {
        None
    };
    info!(
        owner_id = %owner.id, tenant_id, %date, %timezone, limit, ?cursor,
        count = items.len(), has_more, "Policy 日志查询完成"
    );
    Ok(Json(ListPolicyLogsResponse {
        date,
        timezone: timezone.to_string(),
        items,
        next_cursor,
    }))
}
