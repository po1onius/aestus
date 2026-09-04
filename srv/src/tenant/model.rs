use chrono::{DateTime, Utc};
use diesel::prelude::*;
use serde::Serialize;
use uuid::Uuid;

pub mod schema {
    diesel::table! {
        tenants (id) {
            id -> Text,
            enabled -> Bool,
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
}
