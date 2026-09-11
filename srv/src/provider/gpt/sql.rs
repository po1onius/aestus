use chrono::{DateTime, Utc};
use diesel::{
    dsl::sql,
    prelude::*,
    sql_types::{Nullable, Text},
};
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use tracing::info;
use uuid::Uuid;

use crate::{
    err::{AppError, AppResult},
    provider::{
        credential::{
            ACCOUNT_STATUS_VALID, NewProviderAccount, ProviderAccount, schema::provider_accounts,
            serialize_specific,
        },
        gpt::model::{GptAccountSpecific, PROVIDER},
        resource::RequestOverride,
        sql as provider_sql,
    },
};

pub mod account {
    use super::*;

    /// 策略日志只读取本租户 GPT 账号的邮箱投影，不加载 access/refresh token 等凭证。
    pub async fn find_email_by_id(
        conn: &mut AsyncPgConnection,
        tenant_id: &str,
        id: Uuid,
    ) -> AppResult<Option<String>> {
        provider_accounts::table
            .filter(provider_accounts::tenant_id.eq(tenant_id))
            .filter(provider_accounts::provider.eq(PROVIDER))
            .filter(provider_accounts::id.eq(id))
            .select(sql::<Nullable<Text>>("specific ->> 'email'"))
            .first::<Option<String>>(conn)
            .await
            .optional()
            .map(Option::flatten)
            .map_err(|source| AppError::DbQuery {
                message: format!("查询 GPT 账号邮箱失败: {source}"),
            })
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_with_override(
        conn: &mut AsyncPgConnection,
        tenant_id: String,
        chatgpt_account_id: Option<String>,
        email: Option<String>,
        plan_type: String,
        refresh_token: String,
        client_id: String,
        access_token: String,
        next_token_refresh_at: DateTime<Utc>,
        chatgpt_account_is_fedramp: bool,
        request_override: RequestOverride,
        proxy_url: Option<String>,
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
                proxy_url,
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
