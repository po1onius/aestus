use chrono::{DateTime, Utc};
use diesel::prelude::*;
use serde::Serialize;
use uuid::Uuid;

/// 网关用户的 PostgreSQL schema。
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
}

pub const USER_ROLE_PLATFORM_ADMIN: &str = "platform_admin";
pub const USER_ROLE_TENANT_OWNER: &str = "tenant_owner";
pub const USER_ROLE_TENANT_USER: &str = "tenant_user";

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

#[derive(Debug, Clone, Serialize)]
pub struct PublicUser {
    pub id: Uuid,
    pub tenant_id: Option<Uuid>,
    pub username: String,
    pub email: String,
    pub role: String,
    pub quota: i64,
    pub email_verified: bool,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub disabled_at: Option<DateTime<Utc>>,
}

impl From<User> for PublicUser {
    fn from(user: User) -> Self {
        Self {
            id: user.id,
            tenant_id: user.tenant_id,
            username: user.username,
            email: user.email,
            role: user.role,
            quota: user.quota,
            email_verified: user.email_verified,
            enabled: user.enabled,
            created_at: user.created_at,
            updated_at: user.updated_at,
            disabled_at: user.disabled_at,
        }
    }
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

pub(super) fn is_valid_user_role(role: &str) -> bool {
    matches!(
        role,
        USER_ROLE_PLATFORM_ADMIN | USER_ROLE_TENANT_OWNER | USER_ROLE_TENANT_USER
    )
}
