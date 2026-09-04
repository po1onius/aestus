use chrono::{DateTime, Utc};
use diesel::prelude::*;
use serde::Serialize;
use uuid::Uuid;

/// 调用方网关 API Key 的 PostgreSQL schema。
pub mod schema {
    diesel::table! {
        api_keys (id) {
            id -> Uuid,
            tenant_id -> Text,
            user_id -> Uuid,
            group_id -> Uuid,
            name -> Text,
            api_key -> Text,
            plugin_release_id -> Nullable<Uuid>,
            enabled -> Bool,
            created_at -> Timestamptz,
            updated_at -> Timestamptz,
            disabled_at -> Nullable<Timestamptz>,
        }
    }

    diesel::table! {
        api_key_models (api_key_id, model_name) {
            api_key_id -> Uuid,
            model_name -> Text,
            created_at -> Timestamptz,
        }
    }

    diesel::allow_tables_to_appear_in_same_query!(api_keys, api_key_models);
}

/// 调用方网关 API Key 的数据库行。
///
/// 调用方需要在 Dashboard 中随时查看和复制 Key，因此这里保存原始值。HTTP 层使用
/// 专用响应 DTO 明确控制 Key 的返回范围，业务日志则只记录 ID 和名称，禁止记录此字段。
#[derive(Clone, Queryable, Selectable, Serialize)]
#[diesel(table_name = schema::api_keys)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct GatewayApiKey {
    pub id: Uuid,
    pub tenant_id: String,
    pub user_id: Uuid,
    pub group_id: Uuid,
    pub name: String,
    #[serde(skip_serializing)]
    pub api_key: String,
    pub plugin_release_id: Option<Uuid>,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub disabled_at: Option<DateTime<Utc>>,
}

#[derive(Insertable)]
#[diesel(table_name = schema::api_keys)]
pub(super) struct NewGatewayApiKey {
    pub tenant_id: String,
    pub user_id: Uuid,
    pub group_id: Uuid,
    pub name: String,
    pub api_key: String,
    pub plugin_release_id: Option<Uuid>,
}

/// API Key 模型白名单写入行；复合主键在没有外键的前提下负责模型去重。
#[derive(Debug, Insertable)]
#[diesel(table_name = schema::api_key_models)]
pub(super) struct NewGatewayApiKeyModel {
    pub api_key_id: Uuid,
    pub model_name: String,
}
