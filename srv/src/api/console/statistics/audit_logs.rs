//! 控制台请求审计查询：平台全局、owner 本租户、普通用户本人。

use axum::{
    Json, Router,
    extract::{Query, State},
    routing::get,
};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    api::console::auth,
    err::{AppError, AppResult},
    logs::{
        audit::{AuditLogCursor, AuditLogQuery, AuditLogRecord, AuditLogScope, query_audit_logs},
        calendar::{current_service_date, local_day_range_utc},
    },
    state::AppState,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListAuditLogsQuery {
    date: Option<NaiveDate>,
    limit: Option<usize>,
    before_occurred_at: Option<DateTime<Utc>>,
    before_id: Option<Uuid>,
}

#[derive(Serialize)]
struct ListAuditLogsResponse {
    date: NaiveDate,
    timezone: String,
    items: Vec<AuditLogRecord>,
    next_cursor: Option<AuditLogCursor>,
}

pub(super) fn router() -> Router<AppState> {
    Router::new().route("/", get(list_audit_logs))
}

async fn list_audit_logs(
    State(state): State<AppState>,
    auth::CurrentUser(user): auth::CurrentUser,
    Query(params): Query<ListAuditLogsQuery>,
) -> AppResult<Json<ListAuditLogsResponse>> {
    let scope = if user.is_platform_admin() {
        AuditLogScope::Platform
    } else {
        let tenant_id = user.tenant_id.clone().ok_or(AppError::Forbidden)?;
        if user.is_tenant_owner() {
            AuditLogScope::Tenant(tenant_id)
        } else {
            AuditLogScope::User {
                tenant_id,
                user_id: user.id,
            }
        }
    };
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
            message: "不能查询未来日期的审计日志".to_owned(),
        });
    }
    let (start_at, end_at) = local_day_range_utc(timezone, date)?;
    let cursor = match (params.before_occurred_at, params.before_id) {
        (None, None) => None,
        (Some(at), Some(id)) if at >= start_at && at < end_at => Some(AuditLogCursor {
            before_occurred_at: at,
            before_id: id,
        }),
        (Some(_), Some(_)) => {
            return Err(AppError::BadRequest {
                message: "审计日志游标必须位于查询日期内".to_owned(),
            });
        }
        _ => {
            return Err(AppError::BadRequest {
                message: "before_occurred_at 和 before_id 必须同时提供".to_owned(),
            });
        }
    };
    let mut conn = state.db_conn().await?;
    let page = query_audit_logs(
        &mut conn,
        AuditLogQuery {
            scope,
            start_at,
            end_at,
            limit,
            cursor,
        },
    )
    .await?;
    Ok(Json(ListAuditLogsResponse {
        date,
        timezone: timezone.name().to_owned(),
        items: page.items,
        next_cursor: page.next_cursor,
    }))
}
