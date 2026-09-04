use chrono::{DateTime, Utc};
use diesel_async::AsyncPgConnection;
use tracing::info;

use crate::{
    err::AppResult,
    provider::{
        credential::{
            ACCOUNT_STATUS_VALID, NewProviderAccount, ProviderAccount, serialize_specific,
        },
        gpt::model::{GptAccountSpecific, PROVIDER},
        resource::RequestOverride,
        sql as provider_sql,
    },
};

pub mod account {
    use super::*;

    #[allow(clippy::too_many_arguments)]
    pub async fn create_with_override(
        conn: &mut AsyncPgConnection,
        tenant_id: uuid::Uuid,
        chatgpt_account_id: Option<String>,
        email: Option<String>,
        plan_type: String,
        refresh_token: String,
        client_id: String,
        access_token: String,
        next_token_refresh_at: DateTime<Utc>,
        chatgpt_account_is_fedramp: bool,
        request_override: RequestOverride,
    ) -> AppResult<ProviderAccount> {
        let specific = serialize_specific(&GptAccountSpecific {
            chatgpt_account_id,
            email,
            plan_type,
            chatgpt_account_is_fedramp,
        })?;
        let account = provider_sql::account::create(
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
                client_id,
                specific,
                override_: request_override.to_value(),
            },
        )
        .await?;

        info!(
            gpt_account_id = %account.id,
            "GPT 账号已新增；chatgpt_account_id 允许重复"
        );
        Ok(account)
    }
}
