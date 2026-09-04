use chrono::{DateTime, Utc};
use diesel::result::{DatabaseErrorKind, Error as DieselError};
use diesel_async::AsyncPgConnection;
use tracing::{info, warn};

use crate::{
    err::{AppError, AppResult},
    provider::{
        claude::model::{ClaudeAccountSpecific, PROVIDER},
        credential::{
            ACCOUNT_STATUS_VALID, NewProviderAccount, ProviderAccount, serialize_specific,
        },
        resource::RequestOverride,
        sql as provider_sql,
    },
};

pub mod account {
    use super::*;

    pub async fn create(
        conn: &mut AsyncPgConnection,
        tenant_id: uuid::Uuid,
        refresh_token: String,
        access_token: String,
        next_token_refresh_at: DateTime<Utc>,
        specific: ClaudeAccountSpecific,
        request_override: RequestOverride,
    ) -> AppResult<ProviderAccount> {
        let account = provider_sql::account::create_with_db_error_mapper(
            conn,
            NewProviderAccount {
                tenant_id,
                provider: PROVIDER.to_owned(),
                refresh_token,
                access_token,
                credential_generation: 1,
                next_token_refresh_at: Some(next_token_refresh_at),
                quota_resets_at: None,
                enabled: true,
                status: ACCOUNT_STATUS_VALID.to_owned(),
                status_reason: None,
                client_id: crate::provider::claude::auth::CLAUDE_OAUTH_CLIENT_ID.to_owned(),
                specific: serialize_specific(&specific)?,
                override_: request_override.to_value(),
            },
            claude_account_create_db_error,
        )
        .await?;
        info!(
            claude_account_id = %account.id,
            "Claude OAuth 账号已写入通用 provider_accounts"
        );
        Ok(account)
    }
}

/// 将 Claude 专属的 account_uuid 唯一约束转换为管理端可操作的提示。
///
/// 唯一索引属于 Claude 的 `specific` 语义，通用 provider SQL 层只返回普通数据库错误，
/// 避免公共持久化代码反向依赖 Claude 的字段和约束名称。
fn claude_account_create_db_error(source: DieselError) -> AppError {
    const CLAUDE_ACCOUNT_UUID_UNIQUE_INDEX: &str = "uq_provider_accounts_claude_account_uuid";

    if let DieselError::DatabaseError(DatabaseErrorKind::UniqueViolation, information) = &source
        && information.constraint_name() == Some(CLAUDE_ACCOUNT_UUID_UNIQUE_INDEX)
    {
        warn!(
            provider = PROVIDER,
            constraint = CLAUDE_ACCOUNT_UUID_UNIQUE_INDEX,
            "Claude 账号唯一身份冲突，已拒绝重复导入"
        );
        return AppError::BadRequest {
            message: "Claude 账号已导入，不能重复导入同一账号".to_owned(),
        };
    }

    AppError::DbQuery {
        message: source.to_string(),
    }
}
