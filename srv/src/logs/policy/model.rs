//! 策略日志存储模型，供日志模块内部读写使用。

diesel::table! {
    gpt_policy_violation_logs (id) {
        id -> Uuid,
        tenant_id -> Text,
        username -> Text,
        account_email -> Nullable<Text>,
        occurred_at -> Timestamptz,
        error_code -> Text,
    }
}

use chrono::{DateTime, Utc};
use diesel::prelude::*;
use serde::Serialize;
use uuid::Uuid;

#[derive(Queryable, Selectable, Serialize)]
#[diesel(table_name = gpt_policy_violation_logs)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub(crate) struct PolicyLogRecord {
    pub(super) id: Uuid,
    username: String,
    account_email: Option<String>,
    pub(super) occurred_at: DateTime<Utc>,
    error_code: String,
}
