//! 控制台请求审计快照；不关联可变业务表，不保存凭证或正文。

use chrono::{DateTime, Utc};
use diesel::prelude::*;
use serde::Serialize;
use uuid::Uuid;

diesel::table! {
    console_audit_logs (id) {
        id -> Uuid,
        occurred_at -> Timestamptz,
        request_id -> Nullable<Text>,
        user_id -> Nullable<Uuid>,
        username -> Nullable<Text>,
        role -> Nullable<Text>,
        tenant_id -> Nullable<Text>,
        method -> Text,
        path -> Text,
        status_code -> Int4,
        duration_ms -> Int8,
        peer_ip -> Nullable<Text>,
        user_agent -> Nullable<Text>,
    }
}

#[derive(Debug, Clone)]
pub(crate) struct AuditActor {
    pub user_id: Uuid,
    pub username: String,
    pub role: String,
    pub tenant_id: Option<String>,
}

#[derive(Debug, Insertable, Queryable, Selectable, Serialize)]
#[diesel(table_name = console_audit_logs)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub(crate) struct AuditLogRecord {
    pub id: Uuid,
    pub occurred_at: DateTime<Utc>,
    pub request_id: Option<String>,
    pub user_id: Option<Uuid>,
    pub username: Option<String>,
    pub role: Option<String>,
    pub tenant_id: Option<String>,
    pub method: String,
    pub path: String,
    pub status_code: i32,
    pub duration_ms: i64,
    pub peer_ip: Option<String>,
    pub user_agent: Option<String>,
}
