use chrono::{DateTime, Utc};
use diesel_async::AsyncPgConnection;
use tracing::info;

use crate::{
    err::AppResult,
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
        refresh_token: String,
        access_token: String,
        next_token_refresh_at: DateTime<Utc>,
        specific: ClaudeAccountSpecific,
        request_override: RequestOverride,
    ) -> AppResult<ProviderAccount> {
        let account = provider_sql::account::create(
            conn,
            NewProviderAccount {
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
        )
        .await?;
        let specific = account.parse_specific::<ClaudeAccountSpecific>()?;
        info!(
            claude_account_id = %account.id,
            account_uuid = specific.account_uuid.as_deref().unwrap_or("<missing>"),
            organization_uuid = specific.organization_uuid.as_deref().unwrap_or("<missing>"),
            "Claude OAuth 账号已写入通用 provider_accounts"
        );
        Ok(account)
    }
}
