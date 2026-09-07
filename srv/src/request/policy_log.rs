//! 请求策略日志共享表定义，供后台写入与 Dashboard 只读查询使用。

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
