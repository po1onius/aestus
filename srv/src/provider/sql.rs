use chrono::{DateTime, Utc};
use diesel::prelude::*;
use diesel::{
    dsl::{case_when, now},
    pg::expression::extensions::IntervalDsl,
    sql_types::Timestamptz,
};
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl};
use serde_json::Value;
use tracing::{info, warn};
use uuid::Uuid;

use crate::{
    err::{AppError, AppResult},
    provider::{
        credential::{
            ACCOUNT_STATUS_INVALID, ACCOUNT_STATUS_UNAUTHORIZED, ACCOUNT_STATUS_VALID,
            NewProviderAccount, NewProviderApiKey, ProviderAccount, ProviderApiKey,
            is_valid_account_status, normalize_provider,
            schema::{provider_accounts, provider_api_keys},
        },
        group,
        resource::RequestOverride,
    },
};

diesel::define_sql_function! {
    #[sql_name = "GREATEST"]
    fn greatest_timestamptz(left: Timestamptz, right: Timestamptz) -> Timestamptz;
}

diesel::define_sql_function! {
    #[sql_name = "COALESCE"]
    fn coalesce_timestamptz(left: diesel::sql_types::Nullable<Timestamptz>, fallback: Timestamptz) -> Timestamptz;
}

// `updated_at` 只承担 PostgreSQL -> Redis 投影排序，不再作为 token/health 业务 CAS。
// 每次写入至少前进 1 微秒，保证并发管理写和 maintenance 写拥有稳定的全序版本。
macro_rules! next_projection_version {
    ($column:expr) => {
        greatest_timestamptz(now, $column + 1.microseconds())
    };
}

pub mod account {
    use super::*;

    pub async fn create(
        conn: &mut AsyncPgConnection,
        new_account: NewProviderAccount,
    ) -> AppResult<ProviderAccount> {
        create_with_db_error_mapper(conn, new_account, db_error).await
    }

    /// 创建账号并允许 provider facade 自己转换数据库写入错误。
    ///
    /// 通用 SQL 层只负责校验共享字段和执行 insert，不应知道某个 provider 在 `specific`
    /// JSON 上建立了什么唯一索引。需要展示 provider 专属业务提示时，由对应 facade 传入
    /// 错误转换函数；默认的 [`create`] 仍统一折叠为 `DbQuery`。
    pub(crate) async fn create_with_db_error_mapper<F>(
        conn: &mut AsyncPgConnection,
        mut new_account: NewProviderAccount,
        map_db_error: F,
    ) -> AppResult<ProviderAccount>
    where
        F: FnOnce(diesel::result::Error) -> AppError,
    {
        use provider_accounts::dsl;

        new_account.provider = normalize_provider(new_account.provider)?;
        RequestOverride::from_value(new_account.override_.clone())?;
        if !new_account.specific.is_object() {
            return Err(AppError::BadRequest {
                message: "provider account specific 必须是 JSON object".to_owned(),
            });
        }
        if new_account.credential_generation <= 0 {
            return Err(AppError::BadRequest {
                message: "provider account credential_generation 必须大于 0".to_owned(),
            });
        }
        if !is_valid_account_status(&new_account.status) {
            return Err(AppError::BadRequest {
                message: format!("provider 账号凭证状态无效: {}", new_account.status),
            });
        }

        let account = diesel::insert_into(dsl::provider_accounts)
            .values(&new_account)
            .returning(ProviderAccount::as_returning())
            .get_result::<ProviderAccount>(conn)
            .await
            .map_err(map_db_error)?;

        info!(
            provider = %account.provider,
            provider_account_id = %account.id,
            client_id = %account.client_id,
            credential_generation = account.credential_generation,
            "provider 账号凭证已新增；导入后仅允许 maintenance 自发迭代 token 与 specific"
        );
        Ok(account)
    }

    pub async fn list_by_provider(
        conn: &mut AsyncPgConnection,
        provider: &str,
    ) -> AppResult<Vec<ProviderAccount>> {
        use provider_accounts::dsl;

        dsl::provider_accounts
            .filter(dsl::provider.eq(provider))
            .order((dsl::created_at.desc(), dsl::id.desc()))
            .select(ProviderAccount::as_select())
            .load(conn)
            .await
            .map_err(db_error)
    }

    pub async fn list_page_by_provider(
        conn: &mut AsyncPgConnection,
        tenant_id: String,
        provider: &str,
        limit: i64,
        offset: i64,
    ) -> AppResult<Vec<ProviderAccount>> {
        use provider_accounts::dsl;

        dsl::provider_accounts
            .filter(dsl::tenant_id.eq(tenant_id.clone()))
            .filter(dsl::provider.eq(provider))
            .order((dsl::created_at.desc(), dsl::id.desc()))
            .limit(limit)
            .offset(offset)
            .select(ProviderAccount::as_select())
            .load(conn)
            .await
            .map_err(db_error)
    }

    /// 普通用户的资源列表必须先在 PostgreSQL 中按授权分组裁剪，再执行分页。
    pub async fn list_page_by_provider_and_groups(
        conn: &mut AsyncPgConnection,
        tenant_id: String,
        provider: &str,
        group_ids: &[Uuid],
        limit: i64,
        offset: i64,
    ) -> AppResult<Vec<ProviderAccount>> {
        use provider_accounts::dsl;

        dsl::provider_accounts
            .filter(dsl::tenant_id.eq(tenant_id.clone()))
            .filter(dsl::provider.eq(provider))
            .filter(dsl::group_id.eq_any(group_ids))
            .order((dsl::created_at.desc(), dsl::id.desc()))
            .limit(limit)
            .offset(offset)
            .select(ProviderAccount::as_select())
            .load(conn)
            .await
            .map_err(db_error)
    }

    /// 平台管理员按租户查看跨 provider 的账号资源；只读取一页，避免大租户一次加载全部凭证。
    pub async fn list_page_by_tenant(
        conn: &mut AsyncPgConnection,
        tenant_id: String,
        limit: i64,
        offset: i64,
    ) -> AppResult<Vec<ProviderAccount>> {
        use provider_accounts::dsl;

        dsl::provider_accounts
            .filter(dsl::tenant_id.eq(tenant_id.clone()))
            .order((dsl::created_at.desc(), dsl::id.desc()))
            .limit(limit)
            .offset(offset)
            .select(ProviderAccount::as_select())
            .load(conn)
            .await
            .map_err(db_error)
    }

    /// ticker 的唯一 token 刷新扫描入口。管理员关闭调度不会停止 OAuth 凭证维护；
    /// `invalid` 是 refresh token 已确认无效的终态，因此不会继续产生上游刷新请求。
    pub async fn list_due_token_refresh_ids_by_provider(
        conn: &mut AsyncPgConnection,
        provider: &str,
        due_at: DateTime<Utc>,
    ) -> AppResult<Vec<Uuid>> {
        use provider_accounts::dsl;

        dsl::provider_accounts
            .filter(dsl::provider.eq(provider))
            .filter(dsl::status.eq_any([ACCOUNT_STATUS_VALID, ACCOUNT_STATUS_UNAUTHORIZED]))
            .filter(dsl::next_token_refresh_at.le(due_at))
            .order(dsl::next_token_refresh_at.asc())
            .select(dsl::id)
            .load(conn)
            .await
            .map_err(db_error)
    }

    pub async fn list_due_quota_reset_ids_by_provider(
        conn: &mut AsyncPgConnection,
        provider: &str,
        due_at: DateTime<Utc>,
    ) -> AppResult<Vec<Uuid>> {
        use provider_accounts::dsl;

        dsl::provider_accounts
            .filter(dsl::provider.eq(provider))
            .filter(dsl::quota_resets_at.le(due_at))
            .order(dsl::quota_resets_at.asc())
            .select(dsl::id)
            .load(conn)
            .await
            .map_err(db_error)
    }

    pub async fn find_by_id(
        conn: &mut AsyncPgConnection,
        provider: &str,
        id: Uuid,
    ) -> AppResult<Option<ProviderAccount>> {
        use provider_accounts::dsl;

        optional(
            dsl::provider_accounts
                .filter(dsl::provider.eq(provider))
                .filter(dsl::id.eq(id))
                .select(ProviderAccount::as_select())
                .first(conn)
                .await,
        )
    }

    /// 管理员只控制是否参与调度，不允许通过管理接口伪造 credential status。
    pub async fn update_enabled(
        conn: &mut AsyncPgConnection,
        tenant_id: String,
        provider: &str,
        id: Uuid,
        enabled: bool,
    ) -> AppResult<ProviderAccount> {
        use provider_accounts::dsl;

        required_account(
            diesel::update(
                dsl::provider_accounts
                    .filter(dsl::tenant_id.eq(tenant_id.clone()))
                    .filter(dsl::provider.eq(provider))
                    .filter(dsl::id.eq(id)),
            )
            .set((
                dsl::enabled.eq(enabled),
                dsl::updated_at.eq(next_projection_version!(dsl::updated_at)),
            ))
            .returning(ProviderAccount::as_returning())
            .get_result(conn)
            .await,
            provider,
            id,
        )
    }

    pub async fn update_group(
        conn: &mut AsyncPgConnection,
        tenant_id: String,
        provider: &str,
        id: Uuid,
        group_id: Option<Uuid>,
    ) -> AppResult<ProviderAccount> {
        use provider_accounts::dsl;

        conn.transaction::<ProviderAccount, AppError, _>(async |conn| {
            if let Some(group_id) = group_id {
                group::require_enabled_for_provider_write(
                    &mut *conn,
                    tenant_id.clone(),
                    group_id,
                    provider,
                )
                .await?;
            }
            required_account(
                diesel::update(
                    dsl::provider_accounts
                        .filter(dsl::tenant_id.eq(tenant_id.clone()))
                        .filter(dsl::provider.eq(provider))
                        .filter(dsl::id.eq(id)),
                )
                .set((
                    dsl::group_id.eq(group_id),
                    dsl::updated_at.eq(next_projection_version!(dsl::updated_at)),
                ))
                .returning(ProviderAccount::as_returning())
                .get_result(&mut *conn)
                .await,
                provider,
                id,
            )
        })
        .await
    }

    pub async fn delete_by_id(
        conn: &mut AsyncPgConnection,
        tenant_id: String,
        provider: &str,
        id: Uuid,
    ) -> AppResult<ProviderAccount> {
        use provider_accounts::dsl;

        required_account(
            diesel::delete(
                dsl::provider_accounts
                    .filter(dsl::tenant_id.eq(tenant_id.clone()))
                    .filter(dsl::provider.eq(provider))
                    .filter(dsl::id.eq(id)),
            )
            .returning(ProviderAccount::as_returning())
            .get_result(conn)
            .await,
            provider,
            id,
        )
    }

    /// 401 只描述产生该请求的 access token 世代。只有仍为同一 generation 且最近状态为
    /// `valid` 时才首次转入 unauthorized；重复迟到回执不会覆盖 ticker 写入的下一次重试时间。
    pub async fn mark_unauthorized_if_generation_is_valid(
        conn: &mut AsyncPgConnection,
        provider: &str,
        id: Uuid,
        credential_generation: i64,
        reason: String,
        refresh_at: DateTime<Utc>,
    ) -> AppResult<Option<ProviderAccount>> {
        use provider_accounts::dsl;

        optional(
            diesel::update(
                dsl::provider_accounts
                    .filter(dsl::provider.eq(provider))
                    .filter(dsl::id.eq(id))
                    .filter(dsl::credential_generation.eq(credential_generation))
                    .filter(dsl::status.eq(ACCOUNT_STATUS_VALID)),
            )
            .set((
                dsl::status.eq(ACCOUNT_STATUS_UNAUTHORIZED),
                dsl::status_reason.eq(Some(reason)),
                dsl::next_token_refresh_at.eq(Some(refresh_at)),
                dsl::updated_at.eq(next_projection_version!(dsl::updated_at)),
            ))
            .returning(ProviderAccount::as_returning())
            .get_result(conn)
            .await,
        )
    }

    /// quota 属于账号而非某一版 token。多个乱序回执使用 GREATEST 合并，旧回执不能提前
    /// 当前已知的恢复时间，因此不需要绑定 credential generation。quota 状态已经由时间
    /// 完整表达，不写 credential `status_reason`，避免覆盖 unauthorized/invalid 的原因。
    pub async fn extend_quota_reset(
        conn: &mut AsyncPgConnection,
        provider: &str,
        id: Uuid,
        quota_resets_at: DateTime<Utc>,
    ) -> AppResult<Option<ProviderAccount>> {
        use provider_accounts::dsl;

        optional(
            diesel::update(
                dsl::provider_accounts
                    .filter(dsl::provider.eq(provider))
                    .filter(dsl::id.eq(id)),
            )
            .set((
                dsl::quota_resets_at.eq(greatest_timestamptz(
                    coalesce_timestamptz(dsl::quota_resets_at, quota_resets_at),
                    quota_resets_at,
                )
                .nullable()),
                dsl::updated_at.eq(next_projection_version!(dsl::updated_at)),
            ))
            .returning(ProviderAccount::as_returning())
            .get_result(conn)
            .await,
        )
    }

    pub async fn clear_quota_resets_at_if_due(
        conn: &mut AsyncPgConnection,
        provider: &str,
        id: Uuid,
        due_at: DateTime<Utc>,
    ) -> AppResult<Option<ProviderAccount>> {
        use provider_accounts::dsl;

        optional(
            diesel::update(
                dsl::provider_accounts
                    .filter(dsl::provider.eq(provider))
                    .filter(dsl::id.eq(id))
                    .filter(dsl::quota_resets_at.le(due_at)),
            )
            .set((
                dsl::quota_resets_at.eq::<Option<DateTime<Utc>>>(None),
                dsl::updated_at.eq(next_projection_version!(dsl::updated_at)),
            ))
            .returning(ProviderAccount::as_returning())
            .get_result(conn)
            .await,
        )
    }

    /// 管理端主动查询确认额度恢复后，按查询前持久快照清理仍生效的 quota 限制。
    ///
    /// 额度查询包含一次上游网络往返。在此期间可能有真实请求提交新的 quota 回执，或有
    /// maintenance/管理员更新账号。`quota_resets_at + updated_at` 双重 CAS 保证旧查询结果
    /// 不会抹掉这些并发事实；CAS 未命中时由调用方保留当前状态，管理员可再次查询。
    pub async fn clear_quota_resets_at_if_snapshot(
        conn: &mut AsyncPgConnection,
        provider: &str,
        id: Uuid,
        expected_quota_resets_at: DateTime<Utc>,
        expected_updated_at: DateTime<Utc>,
    ) -> AppResult<Option<ProviderAccount>> {
        use provider_accounts::dsl;

        optional(
            diesel::update(
                dsl::provider_accounts
                    .filter(dsl::provider.eq(provider))
                    .filter(dsl::id.eq(id))
                    .filter(dsl::quota_resets_at.eq(expected_quota_resets_at))
                    .filter(dsl::quota_resets_at.gt(now))
                    .filter(dsl::updated_at.eq(expected_updated_at)),
            )
            .set((
                dsl::quota_resets_at.eq::<Option<DateTime<Utc>>>(None),
                dsl::updated_at.eq(next_projection_version!(dsl::updated_at)),
            ))
            .returning(ProviderAccount::as_returning())
            .get_result(conn)
            .await,
        )
    }

    /// refresh 成功只合并 token 世代拥有的字段，并保留管理员并发写入的 enabled/override。
    /// generation CAS 防止已经被新世代替换的 refresh 结果倒灌。
    #[allow(clippy::too_many_arguments)]
    pub async fn record_token_refresh_success_if_generation(
        conn: &mut AsyncPgConnection,
        provider: &str,
        id: Uuid,
        credential_generation: i64,
        refresh_token: String,
        access_token: String,
        next_token_refresh_at: DateTime<Utc>,
        specific: Value,
    ) -> AppResult<Option<ProviderAccount>> {
        use provider_accounts::dsl;

        if !specific.is_object() {
            return Err(AppError::BadRequest {
                message: "provider account specific 必须是 JSON object".to_owned(),
            });
        }
        optional(
            diesel::update(
                dsl::provider_accounts
                    .filter(dsl::provider.eq(provider))
                    .filter(dsl::id.eq(id))
                    .filter(dsl::credential_generation.eq(credential_generation))
                    .filter(
                        dsl::status.eq_any([ACCOUNT_STATUS_VALID, ACCOUNT_STATUS_UNAUTHORIZED]),
                    ),
            )
            .set((
                dsl::refresh_token.eq(refresh_token),
                dsl::access_token.eq(access_token),
                dsl::credential_generation.eq(dsl::credential_generation + 1_i64),
                dsl::next_token_refresh_at.eq(Some(next_token_refresh_at)),
                dsl::specific.eq(specific),
                dsl::status.eq(ACCOUNT_STATUS_VALID),
                dsl::status_reason.eq::<Option<String>>(None),
                dsl::updated_at.eq(next_projection_version!(dsl::updated_at)),
            ))
            .returning(ProviderAccount::as_returning())
            .get_result(conn)
            .await,
        )
    }

    /// refresh 上游临时失败只调整唯一的 next refresh 时间点，保持 valid/unauthorized
    /// 原状态：主动刷新失败可继续使用旧 token，401 后刷新失败则继续保持摘除。
    pub async fn schedule_refresh_retry_if_generation(
        conn: &mut AsyncPgConnection,
        provider: &str,
        id: Uuid,
        credential_generation: i64,
        next_token_refresh_at: DateTime<Utc>,
        reason: String,
    ) -> AppResult<Option<ProviderAccount>> {
        use provider_accounts::dsl;

        optional(
            diesel::update(
                dsl::provider_accounts
                    .filter(dsl::provider.eq(provider))
                    .filter(dsl::id.eq(id))
                    .filter(dsl::credential_generation.eq(credential_generation))
                    .filter(
                        dsl::status.eq_any([ACCOUNT_STATUS_VALID, ACCOUNT_STATUS_UNAUTHORIZED]),
                    ),
            )
            .set((
                dsl::next_token_refresh_at.eq(Some(next_token_refresh_at)),
                dsl::status_reason.eq(Some(reason)),
                dsl::updated_at.eq(next_projection_version!(dsl::updated_at)),
            ))
            .returning(ProviderAccount::as_returning())
            .get_result(conn)
            .await,
        )
    }

    pub async fn mark_invalid_if_generation(
        conn: &mut AsyncPgConnection,
        provider: &str,
        id: Uuid,
        credential_generation: i64,
        reason: String,
    ) -> AppResult<Option<ProviderAccount>> {
        use provider_accounts::dsl;

        optional(
            diesel::update(
                dsl::provider_accounts
                    .filter(dsl::provider.eq(provider))
                    .filter(dsl::id.eq(id))
                    .filter(dsl::credential_generation.eq(credential_generation))
                    .filter(dsl::status.ne(ACCOUNT_STATUS_INVALID)),
            )
            .set((
                dsl::status.eq(ACCOUNT_STATUS_INVALID),
                dsl::status_reason.eq(Some(reason)),
                dsl::next_token_refresh_at.eq::<Option<DateTime<Utc>>>(None),
                dsl::updated_at.eq(next_projection_version!(dsl::updated_at)),
            ))
            .returning(ProviderAccount::as_returning())
            .get_result(conn)
            .await,
        )
    }

    pub async fn update_override(
        conn: &mut AsyncPgConnection,
        tenant_id: String,
        provider: &str,
        id: Uuid,
        request_override: RequestOverride,
    ) -> AppResult<ProviderAccount> {
        use provider_accounts::dsl;

        request_override.validate()?;
        required_account(
            diesel::update(
                dsl::provider_accounts
                    .filter(dsl::tenant_id.eq(tenant_id.clone()))
                    .filter(dsl::provider.eq(provider))
                    .filter(dsl::id.eq(id)),
            )
            .set((
                dsl::override_.eq(request_override.to_value()),
                dsl::updated_at.eq(next_projection_version!(dsl::updated_at)),
            ))
            .returning(ProviderAccount::as_returning())
            .get_result(conn)
            .await,
            provider,
            id,
        )
    }

    /// 委派用户写入覆盖时把鉴权时看到的分组加入 UPDATE 条件，避免 owner 并发换组后
    /// 仍把旧分组权限应用到资源的新安全边界。
    pub async fn update_override_in_group(
        conn: &mut AsyncPgConnection,
        tenant_id: String,
        provider: &str,
        id: Uuid,
        group_id: Uuid,
        request_override: RequestOverride,
    ) -> AppResult<ProviderAccount> {
        use provider_accounts::dsl;

        request_override.validate()?;
        required_account(
            diesel::update(
                dsl::provider_accounts
                    .filter(dsl::tenant_id.eq(tenant_id.clone()))
                    .filter(dsl::provider.eq(provider))
                    .filter(dsl::id.eq(id))
                    .filter(dsl::group_id.eq(group_id)),
            )
            .set((
                dsl::override_.eq(request_override.to_value()),
                dsl::updated_at.eq(next_projection_version!(dsl::updated_at)),
            ))
            .returning(ProviderAccount::as_returning())
            .get_result(conn)
            .await,
            provider,
            id,
        )
    }
}

pub mod api_key {
    use super::*;

    pub async fn create(
        conn: &mut AsyncPgConnection,
        mut new_api_key: NewProviderApiKey,
    ) -> AppResult<ProviderApiKey> {
        use provider_api_keys::dsl;

        new_api_key.provider = normalize_provider(new_api_key.provider)?;
        RequestOverride::from_value(new_api_key.override_.clone())?;
        if new_api_key.error.is_some() != new_api_key.next_probe_at.is_some() {
            return Err(AppError::BadRequest {
                message: "provider API Key error 与 next_probe_at 必须同时为空或同时非空"
                    .to_owned(),
            });
        }

        let api_key = diesel::insert_into(dsl::provider_api_keys)
            .values(&new_api_key)
            .returning(ProviderApiKey::as_returning())
            .get_result::<ProviderApiKey>(conn)
            .await
            .map_err(db_error)?;

        info!(
            provider = %api_key.provider,
            provider_api_key_id = %api_key.id,
            "provider 官方 API Key 已新增；Key 与 Base URL 导入后不可修改"
        );
        Ok(api_key)
    }

    pub async fn list_by_provider(
        conn: &mut AsyncPgConnection,
        provider: &str,
    ) -> AppResult<Vec<ProviderApiKey>> {
        use provider_api_keys::dsl;

        dsl::provider_api_keys
            .filter(dsl::provider.eq(provider))
            .order((dsl::created_at.desc(), dsl::id.desc()))
            .select(ProviderApiKey::as_select())
            .load(conn)
            .await
            .map_err(db_error)
    }

    pub async fn list_page_by_provider(
        conn: &mut AsyncPgConnection,
        tenant_id: String,
        provider: &str,
        limit: i64,
        offset: i64,
    ) -> AppResult<Vec<ProviderApiKey>> {
        use provider_api_keys::dsl;

        dsl::provider_api_keys
            .filter(dsl::tenant_id.eq(tenant_id.clone()))
            .filter(dsl::provider.eq(provider))
            .order((dsl::created_at.desc(), dsl::id.desc()))
            .limit(limit)
            .offset(offset)
            .select(ProviderApiKey::as_select())
            .load(conn)
            .await
            .map_err(db_error)
    }

    /// 普通用户的官方 Key 列表只查询其拥有可视权限的分组，未分组资源始终不可见。
    pub async fn list_page_by_provider_and_groups(
        conn: &mut AsyncPgConnection,
        tenant_id: String,
        provider: &str,
        group_ids: &[Uuid],
        limit: i64,
        offset: i64,
    ) -> AppResult<Vec<ProviderApiKey>> {
        use provider_api_keys::dsl;

        dsl::provider_api_keys
            .filter(dsl::tenant_id.eq(tenant_id.clone()))
            .filter(dsl::provider.eq(provider))
            .filter(dsl::group_id.eq_any(group_ids))
            .order((dsl::created_at.desc(), dsl::id.desc()))
            .limit(limit)
            .offset(offset)
            .select(ProviderApiKey::as_select())
            .load(conn)
            .await
            .map_err(db_error)
    }

    /// 平台管理员按租户查看跨 provider 的官方 API Key；长期 Key 本身不会离开数据库层。
    pub async fn list_page_by_tenant(
        conn: &mut AsyncPgConnection,
        tenant_id: String,
        limit: i64,
        offset: i64,
    ) -> AppResult<Vec<ProviderApiKey>> {
        use provider_api_keys::dsl;

        dsl::provider_api_keys
            .filter(dsl::tenant_id.eq(tenant_id.clone()))
            .order((dsl::created_at.desc(), dsl::id.desc()))
            .limit(limit)
            .offset(offset)
            .select(ProviderApiKey::as_select())
            .load(conn)
            .await
            .map_err(db_error)
    }

    pub async fn list_due_probe_ids_by_provider(
        conn: &mut AsyncPgConnection,
        provider: &str,
        due_at: DateTime<Utc>,
    ) -> AppResult<Vec<Uuid>> {
        use provider_api_keys::dsl;

        dsl::provider_api_keys
            .filter(dsl::provider.eq(provider))
            .filter(dsl::enabled.eq(true))
            .filter(dsl::next_probe_at.le(due_at))
            .order(dsl::next_probe_at.asc())
            .select(dsl::id)
            .load(conn)
            .await
            .map_err(db_error)
    }

    pub async fn find_by_id(
        conn: &mut AsyncPgConnection,
        provider: &str,
        id: Uuid,
    ) -> AppResult<Option<ProviderApiKey>> {
        use provider_api_keys::dsl;

        optional(
            dsl::provider_api_keys
                .filter(dsl::provider.eq(provider))
                .filter(dsl::id.eq(id))
                .select(ProviderApiKey::as_select())
                .first(conn)
                .await,
        )
    }

    pub async fn update_enabled(
        conn: &mut AsyncPgConnection,
        tenant_id: String,
        provider: &str,
        id: Uuid,
        enabled: bool,
        probe_at: DateTime<Utc>,
    ) -> AppResult<ProviderApiKey> {
        use provider_api_keys::dsl;

        // 只有真正的 false -> true 转换才把故障 Key 的探活时间提前。重复提交相同 enabled
        // 不改变 next_probe_at 或投影版本，使 PUT 在业务状态上保持幂等。
        let enabled_changed = dsl::enabled.ne(enabled);
        required_api_key(
            diesel::update(
                dsl::provider_api_keys
                    .filter(dsl::tenant_id.eq(tenant_id.clone()))
                    .filter(dsl::provider.eq(provider))
                    .filter(dsl::id.eq(id)),
            )
            .set((
                dsl::enabled.eq(enabled),
                dsl::next_probe_at.eq(case_when(
                    dsl::enabled
                        .eq(false)
                        .and(enabled_changed)
                        .and(dsl::next_probe_at.is_not_null()),
                    Some(probe_at),
                )
                .otherwise(dsl::next_probe_at)),
                dsl::updated_at.eq(case_when(
                    enabled_changed,
                    next_projection_version!(dsl::updated_at),
                )
                .otherwise(dsl::updated_at)),
            ))
            .returning(ProviderApiKey::as_returning())
            .get_result(conn)
            .await,
            provider,
            id,
        )
    }

    pub async fn update_group(
        conn: &mut AsyncPgConnection,
        tenant_id: String,
        provider: &str,
        id: Uuid,
        group_id: Option<Uuid>,
    ) -> AppResult<ProviderApiKey> {
        use provider_api_keys::dsl;

        conn.transaction::<ProviderApiKey, AppError, _>(async |conn| {
            if let Some(group_id) = group_id {
                group::require_enabled_for_provider_write(
                    &mut *conn,
                    tenant_id.clone(),
                    group_id,
                    provider,
                )
                .await?;
            }
            required_api_key(
                diesel::update(
                    dsl::provider_api_keys
                        .filter(dsl::tenant_id.eq(tenant_id.clone()))
                        .filter(dsl::provider.eq(provider))
                        .filter(dsl::id.eq(id)),
                )
                .set((
                    dsl::group_id.eq(group_id),
                    dsl::updated_at.eq(next_projection_version!(dsl::updated_at)),
                ))
                .returning(ProviderApiKey::as_returning())
                .get_result(&mut *conn)
                .await,
                provider,
                id,
            )
        })
        .await
    }

    /// API Key 不做错误分类。第一个资源级 Error 为健康 Key 写入唯一 Error 和探活时间；
    /// `next_probe_at IS NULL` 直接承担健康 CAS，重复迟到 Error 不覆盖已安排的时间。
    pub async fn record_error_if_healthy(
        conn: &mut AsyncPgConnection,
        provider: &str,
        id: Uuid,
        reason: String,
        probe_at: DateTime<Utc>,
    ) -> AppResult<Option<ProviderApiKey>> {
        use provider_api_keys::dsl;

        optional(
            diesel::update(
                dsl::provider_api_keys
                    .filter(dsl::provider.eq(provider))
                    .filter(dsl::id.eq(id))
                    .filter(dsl::next_probe_at.is_null()),
            )
            .set((
                dsl::error.eq(Some(reason)),
                dsl::next_probe_at.eq(Some(probe_at)),
                dsl::updated_at.eq(next_projection_version!(dsl::updated_at)),
            ))
            .returning(ProviderApiKey::as_returning())
            .get_result(conn)
            .await,
        )
    }

    pub async fn record_probe_success(
        conn: &mut AsyncPgConnection,
        provider: &str,
        id: Uuid,
    ) -> AppResult<Option<ProviderApiKey>> {
        use provider_api_keys::dsl;

        optional(
            diesel::update(
                dsl::provider_api_keys
                    .filter(dsl::provider.eq(provider))
                    .filter(dsl::id.eq(id))
                    .filter(dsl::next_probe_at.is_not_null()),
            )
            .set((
                dsl::error.eq::<Option<String>>(None),
                dsl::next_probe_at.eq::<Option<DateTime<Utc>>>(None),
                dsl::updated_at.eq(next_projection_version!(dsl::updated_at)),
            ))
            .returning(ProviderApiKey::as_returning())
            .get_result(conn)
            .await,
        )
    }

    pub async fn record_probe_failure(
        conn: &mut AsyncPgConnection,
        provider: &str,
        id: Uuid,
        reason: String,
        next_probe_at: DateTime<Utc>,
    ) -> AppResult<Option<ProviderApiKey>> {
        use provider_api_keys::dsl;

        optional(
            diesel::update(
                dsl::provider_api_keys
                    .filter(dsl::provider.eq(provider))
                    .filter(dsl::id.eq(id))
                    .filter(dsl::next_probe_at.is_not_null()),
            )
            .set((
                dsl::error.eq(Some(reason)),
                dsl::next_probe_at.eq(Some(next_probe_at)),
                dsl::updated_at.eq(next_projection_version!(dsl::updated_at)),
            ))
            .returning(ProviderApiKey::as_returning())
            .get_result(conn)
            .await,
        )
    }

    pub async fn update_override(
        conn: &mut AsyncPgConnection,
        tenant_id: String,
        provider: &str,
        id: Uuid,
        request_override: RequestOverride,
    ) -> AppResult<ProviderApiKey> {
        use provider_api_keys::dsl;

        request_override.validate()?;
        required_api_key(
            diesel::update(
                dsl::provider_api_keys
                    .filter(dsl::tenant_id.eq(tenant_id.clone()))
                    .filter(dsl::provider.eq(provider))
                    .filter(dsl::id.eq(id)),
            )
            .set((
                dsl::override_.eq(request_override.to_value()),
                dsl::updated_at.eq(next_projection_version!(dsl::updated_at)),
            ))
            .returning(ProviderApiKey::as_returning())
            .get_result(conn)
            .await,
            provider,
            id,
        )
    }

    pub async fn update_override_in_group(
        conn: &mut AsyncPgConnection,
        tenant_id: String,
        provider: &str,
        id: Uuid,
        group_id: Uuid,
        request_override: RequestOverride,
    ) -> AppResult<ProviderApiKey> {
        use provider_api_keys::dsl;

        request_override.validate()?;
        required_api_key(
            diesel::update(
                dsl::provider_api_keys
                    .filter(dsl::tenant_id.eq(tenant_id.clone()))
                    .filter(dsl::provider.eq(provider))
                    .filter(dsl::id.eq(id))
                    .filter(dsl::group_id.eq(group_id)),
            )
            .set((
                dsl::override_.eq(request_override.to_value()),
                dsl::updated_at.eq(next_projection_version!(dsl::updated_at)),
            ))
            .returning(ProviderApiKey::as_returning())
            .get_result(conn)
            .await,
            provider,
            id,
        )
    }

    pub async fn delete_by_id(
        conn: &mut AsyncPgConnection,
        tenant_id: String,
        provider: &str,
        id: Uuid,
    ) -> AppResult<ProviderApiKey> {
        use provider_api_keys::dsl;

        required_api_key(
            diesel::delete(
                dsl::provider_api_keys
                    .filter(dsl::tenant_id.eq(tenant_id.clone()))
                    .filter(dsl::provider.eq(provider))
                    .filter(dsl::id.eq(id)),
            )
            .returning(ProviderApiKey::as_returning())
            .get_result(conn)
            .await,
            provider,
            id,
        )
    }
}

fn db_error(source: diesel::result::Error) -> AppError {
    AppError::DbQuery {
        message: source.to_string(),
    }
}

fn optional<T>(result: Result<T, diesel::result::Error>) -> AppResult<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(diesel::result::Error::NotFound) => Ok(None),
        Err(source) => Err(db_error(source)),
    }
}

fn required_account(
    result: Result<ProviderAccount, diesel::result::Error>,
    provider: &str,
    id: Uuid,
) -> AppResult<ProviderAccount> {
    match result {
        Ok(value) => Ok(value),
        Err(diesel::result::Error::NotFound) => {
            warn!(provider, provider_account_id = %id, "provider 账号不存在");
            Err(AppError::BadRequest {
                message: format!("provider 账号不存在: provider={provider}, id={id}"),
            })
        }
        Err(source) => Err(db_error(source)),
    }
}

fn required_api_key(
    result: Result<ProviderApiKey, diesel::result::Error>,
    provider: &str,
    id: Uuid,
) -> AppResult<ProviderApiKey> {
    match result {
        Ok(value) => Ok(value),
        Err(diesel::result::Error::NotFound) => {
            warn!(provider, provider_api_key_id = %id, "provider 官方 API Key 不存在");
            Err(AppError::BadRequest {
                message: format!("provider 官方 API Key 不存在: provider={provider}, id={id}"),
            })
        }
        Err(source) => Err(db_error(source)),
    }
}
