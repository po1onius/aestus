//! Policy 日志数据库查询；租户范围必须来自已鉴权的调用方。

use chrono::{DateTime, Utc};
use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use serde::Serialize;
use tracing::{error, info};
use uuid::Uuid;

use super::model::{PolicyLogRecord, gpt_policy_violation_logs};
use crate::err::{AppError, AppResult};

#[derive(Serialize)]
pub(crate) struct PolicyLogCursor {
    before_occurred_at: DateTime<Utc>,
    before_id: Uuid,
}

pub(crate) struct PolicyLogQuery<'a> {
    pub tenant_id: &'a str,
    pub start_at: DateTime<Utc>,
    pub end_at: DateTime<Utc>,
    pub limit: usize,
    pub cursor: Option<(DateTime<Utc>, Uuid)>,
}

pub(crate) struct PolicyLogPage {
    pub items: Vec<PolicyLogRecord>,
    pub next_cursor: Option<PolicyLogCursor>,
}

pub(crate) async fn query_policy_log_page(
    conn: &mut AsyncPgConnection,
    query: PolicyLogQuery<'_>,
) -> AppResult<PolicyLogPage> {
    use gpt_policy_violation_logs::dsl;
    let PolicyLogQuery {
        tenant_id,
        start_at,
        end_at,
        limit,
        cursor,
    } = query;
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
    let mut items = query
        .order((dsl::occurred_at.desc(), dsl::id.desc()))
        .limit((limit + 1) as i64)
        .select(PolicyLogRecord::as_select())
        .load::<PolicyLogRecord>(conn)
        .await
        .map_err(|source| {
            error!(tenant_id, %start_at, %end_at, ?cursor, %source, "查询 Policy 日志失败");
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
    info!(tenant_id, %start_at, %end_at, limit, ?cursor, count = items.len(), has_more, "Policy 日志数据库查询完成");
    Ok(PolicyLogPage { items, next_cursor })
}
