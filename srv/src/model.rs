use chrono::{DateTime, Utc};
use diesel::prelude::*;
use serde::Serialize;
use uuid::Uuid;

/// 网关用户与调用方 API Key 的 PostgreSQL schema。
///
/// 该模块位于 HTTP 层之外，只描述领域持久模型；供应商凭证继续由 provider 的统一
/// credential 模型维护，避免 API handler 承担数据库模型职责。
pub mod schema {
    diesel::table! {
        users (id) {
            id -> Uuid,
            tenant_id -> Nullable<Uuid>,
            username -> Text,
            email -> Text,
            password_hash -> Text,
            role -> Text,
            quota -> Int8,
            consumed_tokens -> Int8,
            email_verified -> Bool,
            enabled -> Bool,
            created_at -> Timestamptz,
            updated_at -> Timestamptz,
            disabled_at -> Nullable<Timestamptz>,
        }
    }

    diesel::table! {
        api_keys (id) {
            id -> Uuid,
            tenant_id -> Uuid,
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

    diesel::allow_tables_to_appear_in_same_query!(users, api_keys, api_key_models,);
}

pub const USER_ROLE_PLATFORM_ADMIN: &str = "platform_admin";
pub const USER_ROLE_TENANT_OWNER: &str = "tenant_owner";
pub const USER_ROLE_TENANT_USER: &str = "tenant_user";

/// API Key 的数据库行。
///
/// 调用方需要在 Dashboard 中随时查看和复制 Key，因此这里保存原始值。HTTP 层使用
/// 专用响应 DTO 明确控制 Key 的返回范围，业务日志则只记录 ID 和名称，禁止记录此字段。
#[derive(Clone, Queryable, Selectable, Serialize)]
#[diesel(table_name = schema::api_keys)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct ApiKey {
    pub id: Uuid,
    pub tenant_id: Uuid,
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
pub(super) struct NewApiKey {
    pub tenant_id: Uuid,
    pub user_id: Uuid,
    pub group_id: Uuid,
    pub name: String,
    pub api_key: String,
    pub plugin_release_id: Option<Uuid>,
}

/// API Key 模型白名单写入行；复合主键在没有外键的前提下负责模型去重。
#[derive(Debug, Insertable)]
#[diesel(table_name = schema::api_key_models)]
pub(super) struct NewApiKeyModel {
    pub api_key_id: Uuid,
    pub model_name: String,
}

/// Dashboard 用户数据库行。
///
/// `quota` 是网关内部剩余 token 额度，`consumed_tokens` 是附属额度功能使用的累计消耗
/// 计数。API Key 只负责访问入口，所有用量最终都归集到用户，避免创建多个 key 后绕过
/// 总额度。
#[derive(Debug, Clone, Queryable, Selectable, Serialize)]
#[diesel(table_name = schema::users)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct User {
    pub id: Uuid,
    pub tenant_id: Option<Uuid>,
    pub username: String,
    pub email: String,
    #[serde(skip_serializing)]
    pub password_hash: String,
    pub role: String,
    pub quota: i64,
    pub consumed_tokens: i64,
    pub email_verified: bool,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub disabled_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Insertable)]
#[diesel(table_name = schema::users)]
pub(super) struct NewUser {
    pub tenant_id: Option<Uuid>,
    pub username: String,
    pub email: String,
    pub password_hash: String,
    pub role: String,
    pub quota: i64,
    pub email_verified: bool,
    pub enabled: bool,
}

#[derive(Debug, AsChangeset)]
#[diesel(table_name = schema::users)]
#[diesel(treat_none_as_null = true)]
pub(super) struct UserStatusPatch {
    pub enabled: bool,
    pub disabled_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

impl User {
    pub fn is_platform_admin(&self) -> bool {
        self.role == USER_ROLE_PLATFORM_ADMIN
    }

    pub fn is_tenant_owner(&self) -> bool {
        self.role == USER_ROLE_TENANT_OWNER
    }
}

pub fn is_valid_user_role(role: &str) -> bool {
    matches!(
        role,
        USER_ROLE_PLATFORM_ADMIN | USER_ROLE_TENANT_OWNER | USER_ROLE_TENANT_USER
    )
}
