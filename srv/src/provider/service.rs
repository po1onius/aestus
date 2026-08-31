use std::marker::PhantomData;

use tracing::{error, info};
use uuid::Uuid;

use crate::{
    err::{AppError, AppResult},
    provider::{
        credential::{NewProviderApiKey, ProviderAccount, ProviderApiKey},
        group::{self, ProviderGroup},
        maintenance::{self, MaintenanceProvider},
        resource::{RequestOverride, UpstreamResourceKind},
        runtime::{self, AccountRuntimeView, ApiKeyRuntimeView, store},
        scheduler, sql,
    },
    state::AppState,
};

/// provider 账号持久模型与当前调度运行态的组合快照。
///
/// service 返回领域数据，HTTP handler 只负责转换为各 provider 的响应 DTO。
pub struct AccountSnapshot {
    pub account: ProviderAccount,
    pub group: Option<ProviderGroup>,
    pub runtime: AccountRuntimeView,
}

/// provider 官方 API Key 持久模型与当前调度运行态的组合快照。
pub struct ApiKeySnapshot {
    pub api_key: ProviderApiKey,
    pub group: Option<ProviderGroup>,
    pub runtime: ApiKeyRuntimeView,
}

/// provider 管理端资源服务。
///
/// 统一封装“PostgreSQL 写入成功后同步 Redis runtime”和“runtime 视图合并 scheduler
/// inflight”两类编排。provider 私有凭证创建/交换仍留在各自模块，本服务不理解 specific。
pub struct ProviderResourceService<'a, P: MaintenanceProvider> {
    state: &'a AppState,
    provider: PhantomData<P>,
}

impl<'a, P: MaintenanceProvider> ProviderResourceService<'a, P> {
    pub fn new(state: &'a AppState) -> Self {
        Self {
            state,
            provider: PhantomData,
        }
    }

    pub async fn find_account(
        &self,
        tenant_id: Uuid,
        id: Uuid,
    ) -> AppResult<Option<ProviderAccount>> {
        let mut conn = self.state.db_conn().await?;
        Ok(sql::account::find_by_id(&mut conn, P::NAME, id)
            .await?
            .filter(|account| account.tenant_id == tenant_id))
    }

    pub async fn list_accounts(
        &self,
        tenant_id: Uuid,
        limit: i64,
        offset: i64,
    ) -> AppResult<Vec<AccountSnapshot>> {
        let mut conn = self.state.db_conn().await?;
        let accounts =
            sql::account::list_page_by_provider(&mut conn, tenant_id, P::NAME, limit, offset)
                .await?;
        drop(conn);
        self.attach_account_runtime(accounts).await
    }

    pub async fn sync_account(&self, account: ProviderAccount) -> AppResult<AccountSnapshot> {
        let id = account.id;
        let result = async {
            maintenance::reconcile_account::<P>(self.state, account.clone()).await?;
            info!(provider = P::NAME, provider_account_id = %id, "provider 账号持久状态已同步到 Redis runtime");
            self.attach_one_account_runtime(account).await
        }
        .await;

        result.map_err(|source| {
            committed_state_sync_error(P::NAME, UpstreamResourceKind::Account, id, source)
        })
    }

    pub async fn update_account_enabled(
        &self,
        tenant_id: Uuid,
        id: Uuid,
        enabled: bool,
    ) -> AppResult<AccountSnapshot> {
        let mut conn = self.state.db_conn().await?;
        require_account_tenant::<P>(&mut conn, tenant_id, id).await?;
        let account =
            sql::account::update_enabled(&mut conn, tenant_id, P::NAME, id, enabled).await?;
        drop(conn);
        self.sync_account(account).await
    }

    pub async fn update_account_override(
        &self,
        tenant_id: Uuid,
        id: Uuid,
        request_override: RequestOverride,
    ) -> AppResult<AccountSnapshot> {
        let mut conn = self.state.db_conn().await?;
        require_account_tenant::<P>(&mut conn, tenant_id, id).await?;
        let account =
            sql::account::update_override(&mut conn, tenant_id, P::NAME, id, request_override)
                .await?;
        drop(conn);
        self.sync_account(account).await
    }

    pub async fn update_account_group(
        &self,
        tenant_id: Uuid,
        id: Uuid,
        group_id: Option<Uuid>,
    ) -> AppResult<AccountSnapshot> {
        let mut conn = self.state.db_conn().await?;
        require_account_tenant::<P>(&mut conn, tenant_id, id).await?;
        let account =
            sql::account::update_group(&mut conn, tenant_id, P::NAME, id, group_id).await?;
        drop(conn);
        self.sync_account(account).await
    }

    /// 额度查询确认恢复后，使用查询前的持久快照 CAS 清理限制并立即恢复 Redis 投影。
    ///
    /// 返回 `None` 表示查询期间账号已经发生并发变化，或限制已自然到期；此时绝不能用
    /// 旧查询结果覆盖新事实。返回 `Some` 时 PostgreSQL 已提交，且最新账号状态已完成
    /// reconcile：只有仍启用、凭证有效且已分组的账号才会重新进入 ready 集合。
    pub async fn clear_account_quota_limit_if_snapshot(
        &self,
        tenant_id: Uuid,
        id: Uuid,
        expected_quota_resets_at: chrono::DateTime<chrono::Utc>,
        expected_updated_at: chrono::DateTime<chrono::Utc>,
    ) -> AppResult<Option<AccountSnapshot>> {
        let mut conn = self.state.db_conn().await?;
        require_account_tenant::<P>(&mut conn, tenant_id, id).await?;
        let account = sql::account::clear_quota_resets_at_if_snapshot(
            &mut conn,
            P::NAME,
            id,
            expected_quota_resets_at,
            expected_updated_at,
        )
        .await?;
        drop(conn);

        match account {
            Some(account) => self.sync_account(account).await.map(Some),
            None => Ok(None),
        }
    }

    /// 数据库删除成功后才写 Redis 永久 tombstone，保持原有 revision fence 语义。
    pub async fn delete_account(&self, tenant_id: Uuid, id: Uuid) -> AppResult<ProviderAccount> {
        let mut conn = self.state.db_conn().await?;
        require_account_tenant::<P>(&mut conn, tenant_id, id).await?;
        let deleted = sql::account::delete_by_id(&mut conn, tenant_id, P::NAME, id).await?;
        drop(conn);
        store::delete_resource(self.state, P::NAME, UpstreamResourceKind::Account, id)
            .await
            .map_err(|source| {
                committed_state_sync_error(P::NAME, UpstreamResourceKind::Account, id, source)
            })?;
        info!(provider = P::NAME, provider_account_id = %id, "provider 账号数据库记录与 Redis runtime 已删除");
        Ok(deleted)
    }

    pub async fn list_api_keys(
        &self,
        tenant_id: Uuid,
        limit: i64,
        offset: i64,
    ) -> AppResult<Vec<ApiKeySnapshot>> {
        let mut conn = self.state.db_conn().await?;
        let api_keys =
            sql::api_key::list_page_by_provider(&mut conn, tenant_id, P::NAME, limit, offset)
                .await?;
        drop(conn);
        self.attach_api_key_runtime(api_keys).await
    }

    /// 创建所有 provider 共用形状的未分组官方 API Key，并在数据库提交后对 Redis 做一次
    /// reconcile，确保旧投影不会残留。provider 私有协议只负责后续请求认证与探活，不为
    /// 相同持久字段复制 SQL facade。
    pub async fn create_api_key(
        &self,
        tenant_id: Uuid,
        api_key: String,
        base_url: String,
        request_override: RequestOverride,
    ) -> AppResult<ApiKeySnapshot> {
        let mut conn = self.state.db_conn().await?;
        let saved = sql::api_key::create(
            &mut conn,
            NewProviderApiKey {
                tenant_id,
                provider: P::NAME.to_owned(),
                api_key,
                base_url,
                enabled: true,
                error: None,
                next_probe_at: None,
                override_: request_override.to_value(),
            },
        )
        .await?;
        drop(conn);
        self.sync_api_key(saved).await
    }

    pub async fn sync_api_key(&self, api_key: ProviderApiKey) -> AppResult<ApiKeySnapshot> {
        let id = api_key.id;
        let result = async {
            maintenance::reconcile_api_key::<P>(self.state, api_key.clone()).await?;
            info!(provider = P::NAME, provider_api_key_id = %id, "provider API Key 持久状态已同步到 Redis runtime");
            self.attach_one_api_key_runtime(api_key).await
        }
        .await;

        result.map_err(|source| {
            committed_state_sync_error(P::NAME, UpstreamResourceKind::ApiKey, id, source)
        })
    }

    pub async fn update_api_key_enabled(
        &self,
        tenant_id: Uuid,
        id: Uuid,
        enabled: bool,
    ) -> AppResult<ApiKeySnapshot> {
        let mut conn = self.state.db_conn().await?;
        require_api_key_tenant::<P>(&mut conn, tenant_id, id).await?;
        let api_key = sql::api_key::update_enabled(
            &mut conn,
            tenant_id,
            P::NAME,
            id,
            enabled,
            chrono::Utc::now(),
        )
        .await?;
        drop(conn);
        self.sync_api_key(api_key).await
    }

    pub async fn update_api_key_override(
        &self,
        tenant_id: Uuid,
        id: Uuid,
        request_override: RequestOverride,
    ) -> AppResult<ApiKeySnapshot> {
        let mut conn = self.state.db_conn().await?;
        require_api_key_tenant::<P>(&mut conn, tenant_id, id).await?;
        let api_key =
            sql::api_key::update_override(&mut conn, tenant_id, P::NAME, id, request_override)
                .await?;
        drop(conn);
        self.sync_api_key(api_key).await
    }

    pub async fn update_api_key_group(
        &self,
        tenant_id: Uuid,
        id: Uuid,
        group_id: Option<Uuid>,
    ) -> AppResult<ApiKeySnapshot> {
        let mut conn = self.state.db_conn().await?;
        require_api_key_tenant::<P>(&mut conn, tenant_id, id).await?;
        let api_key =
            sql::api_key::update_group(&mut conn, tenant_id, P::NAME, id, group_id).await?;
        drop(conn);
        self.sync_api_key(api_key).await
    }

    /// 分组归属事务已经提交后，把受影响资源的最新持久快照统一投射到 Redis。
    ///
    /// 创建分组会把资源发布到 ready index，删除分组则会凭借 `group_id=None` 快照移除
    /// runtime。这里刻意不再查询数据库；即便单个资源同步失败，也继续尝试其余资源，
    /// 尽量缩小 PostgreSQL 已提交但 Redis 尚未同步的范围。
    pub async fn sync_resource_snapshots(
        &self,
        accounts: Vec<ProviderAccount>,
        api_keys: Vec<ProviderApiKey>,
    ) -> AppResult<()> {
        let account_count = accounts.len();
        let upstream_api_key_count = api_keys.len();
        let mut first_error = None;

        for account in accounts {
            let id = account.id;
            if let Err(source) = maintenance::reconcile_account::<P>(self.state, account).await {
                let error =
                    committed_state_sync_error(P::NAME, UpstreamResourceKind::Account, id, source);
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
        for api_key in api_keys {
            let id = api_key.id;
            if let Err(source) = maintenance::reconcile_api_key::<P>(self.state, api_key).await {
                let error =
                    committed_state_sync_error(P::NAME, UpstreamResourceKind::ApiKey, id, source);
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }

        match first_error {
            Some(error) => Err(error),
            None => {
                info!(
                    provider = P::NAME,
                    account_count,
                    upstream_api_key_count,
                    "Provider 分组归属变化涉及的资源已全部同步到 Redis runtime"
                );
                Ok(())
            }
        }
    }

    pub async fn delete_api_key(&self, tenant_id: Uuid, id: Uuid) -> AppResult<ProviderApiKey> {
        let mut conn = self.state.db_conn().await?;
        require_api_key_tenant::<P>(&mut conn, tenant_id, id).await?;
        let deleted = sql::api_key::delete_by_id(&mut conn, tenant_id, P::NAME, id).await?;
        drop(conn);
        store::delete_resource(self.state, P::NAME, UpstreamResourceKind::ApiKey, id)
            .await
            .map_err(|source| {
                committed_state_sync_error(P::NAME, UpstreamResourceKind::ApiKey, id, source)
            })?;
        info!(provider = P::NAME, provider_api_key_id = %id, "provider API Key 数据库记录与 Redis runtime 已删除");
        Ok(deleted)
    }

    async fn attach_one_account_runtime(
        &self,
        account: ProviderAccount,
    ) -> AppResult<AccountSnapshot> {
        let mut snapshots = self.attach_account_runtime(vec![account]).await?;
        Ok(snapshots
            .pop()
            .expect("单个 provider 账号 runtime 合并必须返回一个快照"))
    }

    async fn attach_account_runtime(
        &self,
        accounts: Vec<ProviderAccount>,
    ) -> AppResult<Vec<AccountSnapshot>> {
        let ids = accounts
            .iter()
            .map(|account| account.id)
            .collect::<Vec<_>>();
        let mut runtime_views = runtime::account_views::<P>(self.state, &ids).await?;
        let group_ids = accounts
            .iter()
            .filter_map(|account| account.group_id)
            .collect::<Vec<_>>();
        let mut conn = self.state.db_conn().await?;
        let tenant_id = accounts.first().map(|account| account.tenant_id);
        let groups = match tenant_id {
            Some(tenant_id) => group::find_by_ids(&mut conn, tenant_id, &group_ids).await?,
            None => Default::default(),
        };
        drop(conn);
        let mut load_views =
            scheduler::load_kind_views(self.state, P::NAME, UpstreamResourceKind::Account, &ids)
                .await?;
        accounts
            .into_iter()
            .map(|account| -> AppResult<AccountSnapshot> {
                let group = account
                    .group_id
                    .map(|group_id| {
                        groups.get(&group_id).cloned().ok_or_else(|| AppError::DbQuery {
                            message: format!(
                                "Provider 账号关联的分组不存在: provider={}, account_id={}, group_id={group_id}",
                                P::NAME, account.id
                            ),
                        })
                    })
                    .transpose()?;
                let mut runtime = runtime_views
                    .remove(&account.id)
                    .unwrap_or_else(|| AccountRuntimeView::missing(account.id));
                runtime.inflight_count = load_views
                    .remove(&account.id)
                    .map_or(0, |load| load.inflight_count);
                Ok(AccountSnapshot {
                    account,
                    group,
                    runtime,
                })
            })
            .collect::<AppResult<Vec<_>>>()
    }

    async fn attach_one_api_key_runtime(
        &self,
        api_key: ProviderApiKey,
    ) -> AppResult<ApiKeySnapshot> {
        let mut snapshots = self.attach_api_key_runtime(vec![api_key]).await?;
        Ok(snapshots
            .pop()
            .expect("单个 provider API Key runtime 合并必须返回一个快照"))
    }

    async fn attach_api_key_runtime(
        &self,
        api_keys: Vec<ProviderApiKey>,
    ) -> AppResult<Vec<ApiKeySnapshot>> {
        let ids = api_keys
            .iter()
            .map(|api_key| api_key.id)
            .collect::<Vec<_>>();
        let mut runtime_views = runtime::api_key_views::<P>(self.state, &ids).await?;
        let group_ids = api_keys
            .iter()
            .filter_map(|api_key| api_key.group_id)
            .collect::<Vec<_>>();
        let mut conn = self.state.db_conn().await?;
        let tenant_id = api_keys.first().map(|api_key| api_key.tenant_id);
        let groups = match tenant_id {
            Some(tenant_id) => group::find_by_ids(&mut conn, tenant_id, &group_ids).await?,
            None => Default::default(),
        };
        drop(conn);
        let mut load_views =
            scheduler::load_kind_views(self.state, P::NAME, UpstreamResourceKind::ApiKey, &ids)
                .await?;
        api_keys
            .into_iter()
            .map(|api_key| -> AppResult<ApiKeySnapshot> {
                let group = api_key
                    .group_id
                    .map(|group_id| {
                        groups.get(&group_id).cloned().ok_or_else(|| AppError::DbQuery {
                            message: format!(
                                "Provider 官方 API Key 关联的分组不存在: provider={}, api_key_id={}, group_id={group_id}",
                                P::NAME, api_key.id
                            ),
                        })
                    })
                    .transpose()?;
                let mut runtime = runtime_views
                    .remove(&api_key.id)
                    .unwrap_or_else(|| ApiKeyRuntimeView::missing(api_key.id));
                runtime.inflight_count = load_views
                    .remove(&api_key.id)
                    .map_or(0, |load| load.inflight_count);
                Ok(ApiKeySnapshot {
                    api_key,
                    group,
                    runtime,
                })
            })
            .collect::<AppResult<Vec<_>>>()
    }
}

async fn require_account_tenant<P: MaintenanceProvider>(
    conn: &mut diesel_async::AsyncPgConnection,
    tenant_id: Uuid,
    id: Uuid,
) -> AppResult<()> {
    match sql::account::find_by_id(conn, P::NAME, id).await? {
        Some(account) if account.tenant_id == tenant_id => Ok(()),
        _ => Err(AppError::BadRequest {
            message: format!("Provider 账号不存在: {id}"),
        }),
    }
}

async fn require_api_key_tenant<P: MaintenanceProvider>(
    conn: &mut diesel_async::AsyncPgConnection,
    tenant_id: Uuid,
    id: Uuid,
) -> AppResult<()> {
    match sql::api_key::find_by_id(conn, P::NAME, id).await? {
        Some(api_key) if api_key.tenant_id == tenant_id => Ok(()),
        _ => Err(AppError::BadRequest {
            message: format!("Provider 官方 API Key 不存在: {id}"),
        }),
    }
}

/// PostgreSQL 已经返回成功后，Redis runtime 的写入或读取失败不能再描述成普通失败。
///
/// 调用方重放创建请求可能产生重复凭证，因此统一返回稳定错误码并明确标记数据库已经提交；
/// 不在这里尝试回滚另一套基础设施，也不引入后台补偿状态机。
fn committed_state_sync_error(
    provider: &'static str,
    resource_type: UpstreamResourceKind,
    resource_id: Uuid,
    source: AppError,
) -> AppError {
    error!(
        provider,
        resource_type = resource_type.as_str(),
        resource_id = %resource_id,
        database_committed = true,
        replay_safe = false,
        error = %source,
        "provider 数据库操作已提交，但 Redis runtime 更新或读取失败"
    );
    AppError::ProviderStateSyncFailed {
        provider,
        resource_type: resource_type.as_str(),
        resource_id,
        source: Box::new(source),
    }
}
