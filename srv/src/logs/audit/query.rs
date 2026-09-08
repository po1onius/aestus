//! 按调用方已确认的范围查询审计，不回查用户或租户当前状态。

use chrono::{DateTime, Utc};
use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use serde::Serialize;
use tracing::{error, info};
use uuid::Uuid;

use super::model::{AuditLogRecord, console_audit_logs};
use crate::err::{AppError, AppResult};

/// 显式范围避免普通用户身份缺失时意外退化为全局查询。
#[derive(Debug)]
pub(crate) enum AuditLogScope {
    Platform,
    Tenant(String),
    User { tenant_id: String, user_id: Uuid },
}

#[derive(Debug, Clone, Copy, Serialize)]
pub(crate) struct AuditLogCursor {
    pub before_occurred_at: DateTime<Utc>,
    pub before_id: Uuid,
}

pub(crate) struct AuditLogQuery {
    pub scope: AuditLogScope,
    pub start_at: DateTime<Utc>,
    pub end_at: DateTime<Utc>,
    pub limit: usize,
    pub cursor: Option<AuditLogCursor>,
}

pub(crate) struct AuditLogPage {
    pub items: Vec<AuditLogRecord>,
    pub next_cursor: Option<AuditLogCursor>,
}

pub(crate) async fn query_audit_logs(
    conn: &mut AsyncPgConnection,
    params: AuditLogQuery,
) -> AppResult<AuditLogPage> {
    use console_audit_logs::dsl;
    let mut query = dsl::console_audit_logs
        .filter(dsl::occurred_at.ge(params.start_at))
        .filter(dsl::occurred_at.lt(params.end_at))
        .into_boxed();
    match &params.scope {
        AuditLogScope::Platform => {}
        AuditLogScope::Tenant(tenant_id) => query = query.filter(dsl::tenant_id.eq(tenant_id)),
        AuditLogScope::User { tenant_id, user_id } => {
            query = query
                .filter(dsl::tenant_id.eq(tenant_id))
                .filter(dsl::user_id.eq(user_id));
        }
    }
    if let Some(cursor) = params.cursor {
        query = query.filter(
            dsl::occurred_at
                .lt(cursor.before_occurred_at)
                .or(dsl::occurred_at
                    .eq(cursor.before_occurred_at)
                    .and(dsl::id.lt(cursor.before_id))),
        );
    }
    let mut items = query.order((dsl::occurred_at.desc(), dsl::id.desc()))
        .limit((params.limit + 1) as i64)
        .select(AuditLogRecord::as_select())
        .load::<AuditLogRecord>(conn).await.map_err(|source| {
            error!(scope = ?params.scope, start_at = %params.start_at, end_at = %params.end_at, %source, "控制台审计查询失败");
            AppError::DbQuery { message: format!("查询控制台审计失败: {source}") }
        })?;
    let has_more = items.len() > params.limit;
    items.truncate(params.limit);
    let next_cursor = if has_more {
        items.last().map(|item| AuditLogCursor {
            before_occurred_at: item.occurred_at,
            before_id: item.id,
        })
    } else {
        None
    };
    info!(scope = ?params.scope, start_at = %params.start_at, end_at = %params.end_at, count = items.len(), has_more, "控制台审计查询完成");
    Ok(AuditLogPage { items, next_cursor })
}
