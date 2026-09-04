use chrono::{DateTime, Utc};
use diesel::prelude::*;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use uuid::Uuid;

use crate::{
    err::{AppError, AppResult},
    provider::resource::RequestOverride,
};

/// 所有 provider 共用的持久凭证 schema。
///
/// 长期凭证事实保存在 PostgreSQL；账号的 provider 私有字段放在 `specific`，两类资源的
/// 请求级定制放在 `override`。这些 JSON 在写入前完成强类型校验，热路径不接收任意形状。
pub mod schema {
    diesel::table! {
        provider_accounts (id) {
            id -> Uuid,
            tenant_id -> Text,
            provider -> Text,
            group_id -> Nullable<Uuid>,
            refresh_token -> Text,
            access_token -> Text,
            credential_generation -> Int8,
            next_token_refresh_at -> Nullable<Timestamptz>,
            quota_resets_at -> Nullable<Timestamptz>,
            enabled -> Bool,
            status -> Text,
            status_reason -> Nullable<Text>,
            client_id -> Text,
            specific -> Jsonb,
            #[sql_name = "override"]
            override_ -> Jsonb,
            created_at -> Timestamptz,
            updated_at -> Timestamptz,
        }
    }

    diesel::table! {
        provider_api_keys (id) {
            id -> Uuid,
            tenant_id -> Text,
            provider -> Text,
            group_id -> Nullable<Uuid>,
            api_key -> Text,
            base_url -> Text,
            enabled -> Bool,
            error -> Nullable<Text>,
            next_probe_at -> Nullable<Timestamptz>,
            #[sql_name = "override"]
            override_ -> Jsonb,
            created_at -> Timestamptz,
            updated_at -> Timestamptz,
        }
    }

    diesel::allow_tables_to_appear_in_same_query!(provider_accounts, provider_api_keys);
}

/// 账号凭证最近一次被确认可用。到达主动刷新时间不会改变该状态，旧 token 在刷新成功前
/// 仍可继续服务；只有真实请求返回认证拒绝时才进入 `unauthorized`。
pub const ACCOUNT_STATUS_VALID: &str = "valid";
/// 当前 access token 已被上游拒绝，等待统一 maintenance ticker 使用 refresh token 刷新。
pub const ACCOUNT_STATUS_UNAUTHORIZED: &str = "unauthorized";
/// refresh token 已被上游确认无效。凭证不可编辑，只能删除账号后重新导入。
pub const ACCOUNT_STATUS_INVALID: &str = "invalid";

#[derive(Debug, Clone, Queryable, Selectable, Serialize)]
#[diesel(table_name = schema::provider_accounts)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct ProviderAccount {
    pub id: Uuid,
    pub tenant_id: String,
    pub provider: String,
    /// 未分组资源仍由 maintenance 维护，但不会进入 Redis 调度池。
    pub group_id: Option<Uuid>,
    #[serde(skip_serializing)]
    pub refresh_token: String,
    #[serde(skip_serializing)]
    pub access_token: String,
    /// 只描述 token 世代。管理员 enabled/override、quota 和诊断字段变化不会推进该值。
    pub credential_generation: i64,
    pub next_token_refresh_at: Option<DateTime<Utc>>,
    pub quota_resets_at: Option<DateTime<Utc>>,
    pub enabled: bool,
    pub status: String,
    pub status_reason: Option<String>,
    pub client_id: String,
    pub specific: Value,
    #[serde(rename = "override")]
    pub override_: Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl ProviderAccount {
    pub fn request_override(&self) -> AppResult<RequestOverride> {
        // override 在写入配置时已经完成完整约束校验；持久化读取只恢复强类型结构，
        // 不在 maintenance/runtime 构造链路重复扫描容量与 header 内容。
        parse_provider_json(
            &self.provider,
            "account",
            self.id,
            "override",
            self.override_.clone(),
        )
    }

    pub fn parse_specific<T: DeserializeOwned>(&self) -> AppResult<T> {
        parse_provider_json(
            &self.provider,
            "account",
            self.id,
            "specific",
            self.specific.clone(),
        )
    }
}

#[derive(Debug, Clone, Queryable, Selectable, Serialize)]
#[diesel(table_name = schema::provider_api_keys)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct ProviderApiKey {
    pub id: Uuid,
    pub tenant_id: String,
    pub provider: String,
    /// 官方 Key 可以先导入再由分组创建或迁移操作绑定。
    pub group_id: Option<Uuid>,
    #[serde(skip_serializing)]
    pub api_key: String,
    pub base_url: String,
    pub enabled: bool,
    /// `None` 表示当前健康；出现资源级 Error 或探活失败时保存唯一的原始错误。
    pub error: Option<String>,
    /// `None` 表示健康且无需探活；非空同时表示不可用以及下一次探活时间。
    pub next_probe_at: Option<DateTime<Utc>>,
    #[serde(rename = "override")]
    pub override_: Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl ProviderApiKey {
    pub fn request_override(&self) -> AppResult<RequestOverride> {
        // 与账号保持相同信任边界：数据库只保存经配置写入链路校验过的 override。
        parse_provider_json(
            &self.provider,
            "api_key",
            self.id,
            "override",
            self.override_.clone(),
        )
    }
}

fn parse_provider_json<T: DeserializeOwned>(
    provider: &str,
    kind: &str,
    id: Uuid,
    field: &str,
    value: Value,
) -> AppResult<T> {
    serde_json::from_value(value).map_err(|source| AppError::BadRequest {
        message: format!(
            "provider JSON 字段无法解析: provider={provider}, resource_type={kind}, resource_id={id}, field={field}, error={source}"
        ),
    })
}

#[derive(Debug, Clone, Insertable)]
#[diesel(table_name = schema::provider_accounts)]
pub struct NewProviderAccount {
    pub tenant_id: String,
    pub provider: String,
    pub refresh_token: String,
    pub access_token: String,
    pub credential_generation: i64,
    pub next_token_refresh_at: Option<DateTime<Utc>>,
    pub quota_resets_at: Option<DateTime<Utc>>,
    pub enabled: bool,
    pub status: String,
    pub status_reason: Option<String>,
    pub client_id: String,
    pub specific: Value,
    pub override_: Value,
}

#[derive(Debug, Clone, Insertable)]
#[diesel(table_name = schema::provider_api_keys)]
pub struct NewProviderApiKey {
    pub tenant_id: String,
    pub provider: String,
    pub api_key: String,
    pub base_url: String,
    pub enabled: bool,
    pub error: Option<String>,
    pub next_probe_at: Option<DateTime<Utc>>,
    pub override_: Value,
}

pub fn is_valid_account_status(status: &str) -> bool {
    matches!(
        status,
        ACCOUNT_STATUS_VALID | ACCOUNT_STATUS_UNAUTHORIZED | ACCOUNT_STATUS_INVALID
    )
}

pub fn normalize_provider(provider: String) -> AppResult<String> {
    let provider = provider.trim().to_ascii_lowercase();
    if provider.is_empty() {
        return Err(AppError::BadRequest {
            message: "provider 不能为空".to_owned(),
        });
    }
    Ok(provider)
}

pub fn serialize_specific<T: Serialize>(specific: &T) -> AppResult<Value> {
    let value = serde_json::to_value(specific).map_err(|source| AppError::BadRequest {
        message: format!("provider 私有字段无法序列化: {source}"),
    })?;
    if !value.is_object() {
        return Err(AppError::BadRequest {
            message: "specific 必须序列化为 JSON object".to_owned(),
        });
    }
    Ok(value)
}

/// 将 PostgreSQL 投影更新时间转换为 Redis Lua 可比较的单调版本。
pub fn projection_revision(updated_at: DateTime<Utc>) -> i64 {
    updated_at.timestamp_micros()
}
