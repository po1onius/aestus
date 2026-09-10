use crate::err::{AppError, ConsoleError};
use chrono::{DateTime, Utc};
use diesel::prelude::*;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub mod schema {
    diesel::table! {
        tenants (id) {
            id -> Text,
            enabled -> Bool,
            max_users -> Nullable<Int4>,
            max_gateway_keys_per_user -> Nullable<Int4>,
            owner_can_upload_wasm -> Bool,
            created_by -> Uuid,
            created_at -> Timestamptz,
            updated_at -> Timestamptz,
            disabled_at -> Nullable<Timestamptz>,
        }
    }

    diesel::table! {
        tenant_codes (code) {
            code -> Text,
            tenant_id -> Text,
            created_by -> Uuid,
            created_at -> Timestamptz,
        }
    }

    diesel::allow_tables_to_appear_in_same_query!(tenants, tenant_codes);
}

#[derive(Debug, Clone, Queryable, Selectable, Serialize)]
#[diesel(table_name = schema::tenants)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct Tenant {
    pub id: String,
    pub enabled: bool,
    pub max_users: Option<i32>,
    pub max_gateway_keys_per_user: Option<i32>,
    pub owner_can_upload_wasm: bool,
    pub created_by: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub disabled_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TenantSummary {
    #[serde(flatten)]
    pub tenant: Tenant,
    pub code: Option<String>,
    pub user_count: i64,
}

/// 数量为 NULL 表示不限制，0 表示禁止新增；更新接口按整组替换。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TenantLimits {
    pub max_users: Option<i32>,
    pub max_gateway_keys_per_user: Option<i32>,
    pub owner_can_upload_wasm: bool,
}

impl TenantLimits {
    pub fn validate(&self) -> crate::err::AppResult<()> {
        if self.max_users.is_some_and(|value| value < 0)
            || self.max_gateway_keys_per_user.is_some_and(|value| value < 0)
        {
            return Err(crate::err::AppError::BadRequest {
                message: "数量上限必须为 0..=2147483647 的整数，或 null（不限制）".to_owned(),
            });
        }
        Ok(())
    }
}

impl Tenant {
    pub fn limits(&self) -> TenantLimits {
        TenantLimits {
            max_users: self.max_users,
            max_gateway_keys_per_user: self.max_gateway_keys_per_user,
            owner_can_upload_wasm: self.owner_can_upload_wasm,
        }
    }

    pub fn require_wasm_upload(&self, actor_id: Uuid) -> crate::err::AppResult<()> {
        if !self.owner_can_upload_wasm {
            tracing::warn!(tenant_id = %self.id, %actor_id, "租户未获授权上传 WASM 插件");
            return Err(AppError::Console(ConsoleError::TenantWasmUploadForbidden));
        }
        Ok(())
    }
}
