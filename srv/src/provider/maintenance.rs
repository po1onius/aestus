use std::{collections::HashSet, future::Future};

use chrono::{DateTime, Utc};
use tokio::task::{JoinError, JoinHandle, JoinSet};
use tokio::time::{Duration, MissedTickBehavior, interval, timeout};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::{
    err::{AppError, AppResult},
    provider::{
        credential::{
            ACCOUNT_STATUS_INVALID, ACCOUNT_STATUS_UNAUTHORIZED, ACCOUNT_STATUS_VALID,
            ProviderAccount, ProviderApiKey, projection_revision,
        },
        protocol::UpstreamFeedback,
        resource::{UpstreamResource, UpstreamResourceKind},
        runtime::store,
        sql,
    },
    state::AppState,
};

const MAINTENANCE_INTERVAL_SECONDS: u64 = 15;
const MAINTENANCE_JOB_TIMEOUT_GRACE_SECONDS: u64 = 30;
const MAINTENANCE_LOCK_RELEASE_TIMEOUT_SECONDS: u64 = 5;
const MAINTENANCE_LOCK_TTL_GRACE_SECONDS: u64 = 60;

/// provider 只实现凭证协议差异。通用 maintenance 独占 token 刷新与 API Key 探活的
/// 执行入口，并统一掌握 ticker、资源锁、持久状态迁移和 Redis 投影。
pub trait MaintenanceProvider: Send + Sync + 'static {
    const NAME: &'static str;

    fn account_request_context(account: &ProviderAccount) -> AppResult<serde_json::Value>;

    fn refresh_account<'a>(
        state: &'a AppState,
        account: &'a ProviderAccount,
    ) -> impl Future<Output = Result<RefreshedAccount, MaintenanceFailure>> + Send + 'a;

    fn probe_api_key<'a>(
        state: &'a AppState,
        api_key: &'a ProviderApiKey,
    ) -> impl Future<Output = Result<(), MaintenanceFailure>> + Send + 'a;

    /// refresh endpoint 的所有非终态失败使用 Provider 独立的固定重试间隔，并复用账号
    /// 唯一的 next refresh 时间点；业务请求的限流与 5xx 冷却配置不得影响凭证维护节奏。
    fn account_refresh_retry_seconds(state: &AppState) -> u64;

    /// API Key 只有一个 next probe 时间点，不根据错误类型计算 quota/cooldown。
    fn api_key_probe_interval_seconds(state: &AppState) -> u64;

    /// 将 provider protocol 识别出的上游事实映射成最小维护命令。API Key 无论收到哪种
    /// 资源级反馈都必须映射为同一个 `ApiKeyError`，maintenance 不再分类其错误原因。
    fn feedback_command(
        state: &AppState,
        request_id: Uuid,
        resource: &UpstreamResource,
        feedback: UpstreamFeedback,
    ) -> AppResult<Option<ResourceFeedback>>;
}

#[derive(Debug, Clone)]
pub struct RefreshedAccount {
    pub refresh_token: String,
    pub access_token: String,
    pub next_token_refresh_at: DateTime<Utc>,
    pub specific: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaintenanceFailureKind {
    Unauthorized,
    RateLimited,
    Retryable,
    BadResponse,
}

impl MaintenanceFailureKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unauthorized => "unauthorized",
            Self::RateLimited => "rate_limited",
            Self::Retryable => "retryable",
            Self::BadResponse => "bad_response",
        }
    }
}

#[derive(Debug, Clone)]
pub struct MaintenanceFailure {
    pub kind: MaintenanceFailureKind,
    pub message: String,
}

impl std::fmt::Display for MaintenanceFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

/// 请求链路提交给通用 maintenance 的最小资源级事实。
#[derive(Debug, Clone)]
pub enum ResourceFeedback {
    AccountUnauthorized { reason: String },
    AccountQuotaLimited { resets_at: DateTime<Utc> },
    ApiKeyError { reason: String },
}

impl ResourceFeedback {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::AccountUnauthorized { .. } => "account_unauthorized",
            Self::AccountQuotaLimited { .. } => "account_quota_limited",
            Self::ApiKeyError { .. } => "api_key_error",
        }
    }
}

struct MaintenanceJobCompletion {
    resource_kind: UpstreamResourceKind,
    resource_id: Uuid,
    result: AppResult<()>,
}

pub(super) fn spawn_maintenance_loop<P: MaintenanceProvider>(state: AppState) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = interval(Duration::from_secs(MAINTENANCE_INTERVAL_SECONDS));
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut jobs = JoinSet::<MaintenanceJobCompletion>::new();

        info!(
            provider = P::NAME,
            maintenance_interval_seconds = MAINTENANCE_INTERVAL_SECONDS,
            maintenance_job_timeout_seconds = maintenance_job_timeout_seconds(&state),
            maintenance_lock_ttl_seconds = maintenance_lock_ttl_seconds(&state),
            "provider maintenance ticker 已启动；token 刷新与 API Key 探活仅由该循环提交"
        );

        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    if let Err(error) = dispatch_maintenance_once::<P>(&state, &mut jobs).await {
                        error!(provider = P::NAME, error = %error, "provider 上游资源维护调度失败");
                    }
                }
                Some(completion) = jobs.join_next(), if !jobs.is_empty() => {
                    log_maintenance_job_completion::<P>(completion);
                }
            }
        }
    })
}

async fn dispatch_maintenance_once<P: MaintenanceProvider>(
    state: &AppState,
    jobs: &mut JoinSet<MaintenanceJobCompletion>,
) -> AppResult<()> {
    let account_jobs = dispatch_due_account_jobs::<P>(state, jobs).await?;
    let api_key_jobs = dispatch_due_api_key_jobs::<P>(state, jobs).await?;
    let submitted_jobs = account_jobs + api_key_jobs;

    if submitted_jobs == 0 {
        debug!(
            provider = P::NAME,
            active_maintenance_jobs = jobs.len(),
            "本轮 provider maintenance 没有到期资源"
        );
    } else {
        info!(
            provider = P::NAME,
            submitted_account_jobs = account_jobs,
            submitted_api_key_jobs = api_key_jobs,
            active_maintenance_jobs = jobs.len(),
            "本轮 provider maintenance 非阻塞任务已提交"
        );
    }
    Ok(())
}

fn log_maintenance_job_completion<P: MaintenanceProvider>(
    completion: Result<MaintenanceJobCompletion, JoinError>,
) {
    match completion {
        Ok(MaintenanceJobCompletion {
            resource_kind,
            resource_id,
            result: Ok(()),
        }) => info!(
            provider = P::NAME,
            resource_type = resource_kind.as_str(),
            resource_id = %resource_id,
            "provider maintenance 后台任务完成"
        ),
        Ok(MaintenanceJobCompletion {
            resource_kind,
            resource_id,
            result: Err(error),
        }) => error!(
            provider = P::NAME,
            resource_type = resource_kind.as_str(),
            resource_id = %resource_id,
            error = %error,
            "provider maintenance 后台任务失败"
        ),
        Err(error) => error!(
            provider = P::NAME,
            task_cancelled = error.is_cancelled(),
            task_panicked = error.is_panic(),
            error = %error,
            "provider maintenance 后台任务异常结束"
        ),
    }
}

/// 服务启动只按 PostgreSQL 当前事实重建 Redis 投影，不发起 token refresh 或 API Key
/// probe。所有到期工作留给随后启动的统一 ticker，启动路径不会被 provider 网络阻塞。
pub(super) async fn bootstrap_provider_runtime<P: MaintenanceProvider>(
    state: &AppState,
) -> AppResult<usize> {
    store::clear_runtime_index(state, P::NAME).await?;

    let mut conn = state.db_conn().await?;
    let accounts = sql::account::list_by_provider(&mut conn, P::NAME).await?;
    let api_keys = sql::api_key::list_by_provider(&mut conn, P::NAME).await?;
    drop(conn);

    let mut ready_count = 0usize;
    for account in accounts {
        match reconcile_account::<P>(state, account).await {
            Ok(true) => ready_count += 1,
            Ok(false) => {}
            Err(error) => {
                warn!(provider = P::NAME, error = %error, "provider 账号 runtime 重建失败，已跳过")
            }
        }
    }
    for api_key in api_keys {
        match reconcile_api_key::<P>(state, api_key).await {
            Ok(true) => ready_count += 1,
            Ok(false) => {}
            Err(error) => {
                warn!(provider = P::NAME, error = %error, "provider API Key runtime 重建失败，已跳过")
            }
        }
    }

    info!(
        provider = P::NAME,
        ready_runtime_count = ready_count,
        "provider 上游资源 runtime 重建完成"
    );
    Ok(ready_count)
}

/// 账号投影依据分组归属、enabled、credential status 与 quota reset，不依据 next refresh
/// 时间。未分组账号仍会被 ticker 刷新，但不会进入可调度池；到达主动刷新时间的 valid
/// token 会继续服务，ticker 成功取得新 token 后再原子替换。
pub async fn reconcile_account<P: MaintenanceProvider>(
    state: &AppState,
    account: ProviderAccount,
) -> AppResult<bool> {
    ensure_provider::<P>(&account.provider)?;
    if account.group_id.is_some() && account_is_ready(&account) {
        return store::publish(state, resource_from_account::<P>(&account)?).await;
    }
    remove_account_runtime::<P>(state, &account).await?;
    Ok(false)
}

pub async fn reconcile_api_key<P: MaintenanceProvider>(
    state: &AppState,
    api_key: ProviderApiKey,
) -> AppResult<bool> {
    ensure_provider::<P>(&api_key.provider)?;
    if api_key.group_id.is_some() && api_key.enabled && api_key.next_probe_at.is_none() {
        return store::publish(state, resource_from_api_key::<P>(&api_key)?).await;
    }
    remove_api_key_runtime::<P>(state, &api_key).await?;
    Ok(false)
}

/// 请求链路同步完成数据库状态迁移和 Redis 隔离，但绝不调用 provider refresh/probe。
/// 因此重试前 ready 集合已经反映故障，而上游维护网络请求只可能由 ticker 提交。
pub async fn handle_feedback<P: MaintenanceProvider>(
    state: &AppState,
    request_id: Uuid,
    runtime: &UpstreamResource,
    feedback: ResourceFeedback,
) -> AppResult<()> {
    ensure_provider::<P>(&runtime.provider)?;
    let feedback_kind = feedback.as_str();
    match feedback {
        ResourceFeedback::AccountUnauthorized { reason } => {
            ensure_kind(runtime, UpstreamResourceKind::Account)?;
            handle_account_unauthorized::<P>(state, request_id, runtime, reason).await?
        }
        ResourceFeedback::AccountQuotaLimited { resets_at } => {
            ensure_kind(runtime, UpstreamResourceKind::Account)?;
            handle_account_quota::<P>(state, request_id, runtime, resets_at).await?
        }
        ResourceFeedback::ApiKeyError { reason } => {
            ensure_kind(runtime, UpstreamResourceKind::ApiKey)?;
            handle_api_key_error::<P>(state, request_id, runtime, reason).await?
        }
    }

    info!(
        request_id = %request_id,
        provider = P::NAME,
        resource_type = runtime.kind.as_str(),
        resource_id = %runtime.id,
        projection_version = runtime.revision,
        credential_generation = ?runtime.credential_generation,
        feedback = feedback_kind,
        "请求回执 maintenance 状态迁移与 runtime 隔离已完成"
    );
    Ok(())
}

async fn handle_account_unauthorized<P: MaintenanceProvider>(
    state: &AppState,
    request_id: Uuid,
    runtime: &UpstreamResource,
    reason: String,
) -> AppResult<()> {
    let generation = runtime
        .credential_generation
        .ok_or_else(|| AppError::BadRequest {
            message: format!("账号 runtime 缺少 credential_generation: {}", runtime.id),
        })?;
    let reason = truncate_diagnostic(&reason);
    let mut conn = state.db_conn().await?;
    let updated = sql::account::mark_unauthorized_if_generation_is_valid(
        &mut conn,
        P::NAME,
        runtime.id,
        generation,
        reason,
        Utc::now(),
    )
    .await?;

    if let Some(updated) = updated {
        drop(conn);
        remove_account_runtime::<P>(state, &updated).await?;
        info!(
            request_id = %request_id,
            provider = P::NAME,
            provider_account_id = %runtime.id,
            credential_generation = generation,
            next_token_refresh_at = ?updated.next_token_refresh_at,
            "当前 token 世代已标记 unauthorized，等待 maintenance ticker 刷新"
        );
        return Ok(());
    }

    let current = sql::account::find_by_id(&mut conn, P::NAME, runtime.id).await?;
    drop(conn);
    let stale = current
        .as_ref()
        .is_none_or(|account| account.credential_generation != generation);
    // 同一 token 的多个在途请求可能同时返回 401。第一个请求已经完成状态迁移但尚未执行
    // Redis 摘除时，后续请求也必须幂等地摘除同一新投影，不能依赖第一个任务的时序。
    if let Some(current) = current.as_ref()
        && current.credential_generation == generation
        && current.status != ACCOUNT_STATUS_VALID
    {
        remove_account_runtime::<P>(state, current).await?;
    }
    info!(
        request_id = %request_id,
        provider = P::NAME,
        provider_account_id = %runtime.id,
        feedback_generation = generation,
        current_generation = current.as_ref().map(|account| account.credential_generation),
        current_status = current.as_ref().map(|account| account.status.as_str()),
        stale,
        "账号 unauthorized 回执未触发重复状态迁移"
    );
    Ok(())
}

async fn handle_account_quota<P: MaintenanceProvider>(
    state: &AppState,
    request_id: Uuid,
    runtime: &UpstreamResource,
    resets_at: DateTime<Utc>,
) -> AppResult<()> {
    let mut conn = state.db_conn().await?;
    let updated =
        sql::account::extend_quota_reset(&mut conn, P::NAME, runtime.id, resets_at).await?;
    drop(conn);
    if let Some(updated) = updated {
        remove_account_runtime::<P>(state, &updated).await?;
        info!(
            request_id = %request_id,
            provider = P::NAME,
            provider_account_id = %runtime.id,
            quota_resets_at = ?updated.quota_resets_at,
            "账号 quota reset 已按最晚时间合并并从 ready 集合摘除"
        );
    }
    Ok(())
}

async fn handle_api_key_error<P: MaintenanceProvider>(
    state: &AppState,
    request_id: Uuid,
    runtime: &UpstreamResource,
    reason: String,
) -> AppResult<()> {
    let reason = truncate_diagnostic(&reason);
    let mut conn = state.db_conn().await?;
    let updated =
        sql::api_key::record_error_if_healthy(&mut conn, P::NAME, runtime.id, reason, Utc::now())
            .await?;
    if let Some(updated) = updated {
        drop(conn);
        remove_api_key_runtime::<P>(state, &updated).await?;
        info!(
            request_id = %request_id,
            provider = P::NAME,
            provider_api_key_id = %runtime.id,
            next_probe_at = ?updated.next_probe_at,
            "API Key 原始 Error 与 next_probe_at 已写入，等待 ticker 探活"
        );
        return Ok(());
    }

    let current = sql::api_key::find_by_id(&mut conn, P::NAME, runtime.id).await?;
    drop(conn);
    // 多个在途请求可以同时返回 Error。首个回执写入 next_probe_at 后、摘除 Redis 前，
    // 后续回执也主动摘除当前投影，避免把立即隔离依赖在某一个请求的收尾时序上。
    if let Some(current) = current.as_ref()
        && current.next_probe_at.is_some()
    {
        remove_api_key_runtime::<P>(state, current).await?;
    }
    debug!(
        request_id = %request_id,
        provider = P::NAME,
        provider_api_key_id = %runtime.id,
        probe_pending = current.as_ref().is_some_and(|api_key| api_key.next_probe_at.is_some()),
        "API Key 已在等待探活或已删除，重复 Error 未覆盖下一次探活时间"
    );
    Ok(())
}

/// 将 provider 事实映射并应用到持久状态。返回值只表示是否存在资源级维护命令，供日志
/// 展示使用；请求是否重试完全由 proxy 的响应分类决定，不由 maintenance 返回决策。
pub async fn apply_upstream_feedback<P: MaintenanceProvider>(
    state: &AppState,
    request_id: Uuid,
    resource: &UpstreamResource,
    feedback: UpstreamFeedback,
) -> AppResult<bool> {
    ensure_provider::<P>(&resource.provider)?;
    let feedback_kind = feedback.as_str();
    let Some(command) = P::feedback_command(state, request_id, resource, feedback)? else {
        info!(
            request_id = %request_id,
            provider = P::NAME,
            resource_type = resource.kind.as_str(),
            resource_id = %resource.id,
            feedback = feedback_kind,
            "上游事实没有可靠资源级语义，不改变 maintenance 状态"
        );
        return Ok(false);
    };
    handle_feedback::<P>(state, request_id, resource, command).await?;
    Ok(true)
}

/// token refresh 和 quota reset 同属账号维护，按账号 ID 合并后共用一把资源锁和一个
/// ticker 任务，确保 refresh 前读取的是最新持久状态。
async fn dispatch_due_account_jobs<P: MaintenanceProvider>(
    state: &AppState,
    jobs: &mut JoinSet<MaintenanceJobCompletion>,
) -> AppResult<usize> {
    let now = Utc::now();
    let mut conn = state.db_conn().await?;
    let refresh_ids =
        sql::account::list_due_token_refresh_ids_by_provider(&mut conn, P::NAME, now).await?;
    let quota_ids =
        sql::account::list_due_quota_reset_ids_by_provider(&mut conn, P::NAME, now).await?;
    drop(conn);

    let refresh_count = refresh_ids.len();
    let quota_count = quota_ids.len();
    let mut seen = HashSet::with_capacity(refresh_count + quota_count);
    let mut due_ids = Vec::with_capacity(refresh_count + quota_count);
    for id in refresh_ids.into_iter().chain(quota_ids) {
        if seen.insert(id) {
            due_ids.push(id);
        }
    }

    let mut submitted = 0usize;
    for id in due_ids {
        let Some(lock) = store::try_resource_lock(
            state,
            P::NAME,
            UpstreamResourceKind::Account,
            id,
            maintenance_lock_ttl_seconds(state),
        )
        .await?
        else {
            debug!(provider = P::NAME, provider_account_id = %id, "账号 maintenance 锁已被持有，本轮不重复提交");
            continue;
        };

        let task_state = state.clone();
        jobs.spawn(async move {
            let result = run_account_maintenance_with_lock::<P>(&task_state, id, lock)
                .await
                .map(|_| ());
            MaintenanceJobCompletion {
                resource_kind: UpstreamResourceKind::Account,
                resource_id: id,
                result,
            }
        });
        submitted += 1;
    }

    debug!(
        provider = P::NAME,
        due_token_refresh_count = refresh_count,
        due_quota_reset_count = quota_count,
        distinct_due_account_count = seen.len(),
        submitted_account_jobs = submitted,
        "账号 maintenance 到期扫描完成"
    );
    Ok(submitted)
}

async fn dispatch_due_api_key_jobs<P: MaintenanceProvider>(
    state: &AppState,
    jobs: &mut JoinSet<MaintenanceJobCompletion>,
) -> AppResult<usize> {
    let mut conn = state.db_conn().await?;
    let due_ids =
        sql::api_key::list_due_probe_ids_by_provider(&mut conn, P::NAME, Utc::now()).await?;
    drop(conn);
    let due_count = due_ids.len();
    let mut submitted = 0usize;

    for id in due_ids {
        let Some(lock) = store::try_resource_lock(
            state,
            P::NAME,
            UpstreamResourceKind::ApiKey,
            id,
            maintenance_lock_ttl_seconds(state),
        )
        .await?
        else {
            debug!(provider = P::NAME, provider_api_key_id = %id, "API Key maintenance 锁已被持有，本轮不重复提交");
            continue;
        };

        let task_state = state.clone();
        jobs.spawn(async move {
            let result = run_api_key_maintenance_with_lock::<P>(&task_state, id, lock).await;
            MaintenanceJobCompletion {
                resource_kind: UpstreamResourceKind::ApiKey,
                resource_id: id,
                result,
            }
        });
        submitted += 1;
    }

    debug!(
        provider = P::NAME,
        due_api_key_probe_count = due_count,
        submitted_api_key_jobs = submitted,
        "API Key maintenance 到期扫描完成"
    );
    Ok(submitted)
}

async fn run_account_maintenance_with_lock<P: MaintenanceProvider>(
    state: &AppState,
    id: Uuid,
    lock: store::LockToken,
) -> AppResult<Option<UpstreamResource>> {
    let timeout_seconds = maintenance_job_timeout_seconds(state);
    let result = match timeout(
        Duration::from_secs(timeout_seconds),
        maintain_account_with_lock::<P>(state, id),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => Err(maintenance_timeout_error::<P>(
            UpstreamResourceKind::Account,
            id,
            timeout_seconds,
        )),
    };
    release_maintenance_lock::<P>(state, UpstreamResourceKind::Account, id, lock).await;
    result
}

/// ticker 锁内先清理到期 quota，再处理唯一的 next refresh 时间。valid 账号主动刷新时不
/// 摘除旧 token；unauthorized 账号保持摘除，直到 refresh 成功产生新 generation。
async fn maintain_account_with_lock<P: MaintenanceProvider>(
    state: &AppState,
    id: Uuid,
) -> AppResult<Option<UpstreamResource>> {
    let mut conn = state.db_conn().await?;
    let Some(account) = sql::account::find_by_id(&mut conn, P::NAME, id).await? else {
        store::delete_resource(state, P::NAME, UpstreamResourceKind::Account, id).await?;
        return Ok(None);
    };
    drop(conn);

    let quota_was_due = account
        .quota_resets_at
        .is_some_and(|quota_resets_at| quota_resets_at <= Utc::now());
    let Some(account) = clear_expired_quota_if_needed::<P>(state, account).await? else {
        debug!(
            provider = P::NAME,
            provider_account_id = %id,
            "账号 quota reset 清理 CAS 未命中，持久状态已并发变化，本轮 maintenance 已停止"
        );
        return Ok(None);
    };
    if quota_was_due && account.quota_resets_at.is_none() {
        info!(
            provider = P::NAME,
            provider_account_id = %id,
            projection_version = %account.updated_at,
            "账号 quota reset 已由 ticker 清理"
        );
    }

    if account.status == ACCOUNT_STATUS_INVALID {
        remove_account_runtime::<P>(state, &account).await?;
        return Ok(None);
    }

    let refresh_due = account
        .next_token_refresh_at
        .is_some_and(|next| next <= Utc::now());
    if !refresh_due {
        return publish_account_if_ready::<P>(state, account).await;
    }

    if account.status == ACCOUNT_STATUS_UNAUTHORIZED {
        remove_account_runtime::<P>(state, &account).await?;
    }
    let generation = account.credential_generation;
    info!(
        provider = P::NAME,
        provider_account_id = %id,
        credential_generation = generation,
        credential_status = %account.status,
        enabled = account.enabled,
        "maintenance ticker 开始执行 provider token refresh"
    );

    match P::refresh_account(state, &account).await {
        Ok(refreshed) => {
            let mut conn = state.db_conn().await?;
            let updated = sql::account::record_token_refresh_success_if_generation(
                &mut conn,
                P::NAME,
                id,
                generation,
                refreshed.refresh_token,
                refreshed.access_token,
                refreshed.next_token_refresh_at,
                refreshed.specific,
            )
            .await?;
            drop(conn);
            let Some(updated) = updated else {
                info!(
                    provider = P::NAME,
                    provider_account_id = %id,
                    credential_generation = generation,
                    "token refresh 结果不属于当前凭证世代，已忽略"
                );
                return Ok(None);
            };
            info!(
                provider = P::NAME,
                provider_account_id = %id,
                previous_generation = generation,
                credential_generation = updated.credential_generation,
                next_token_refresh_at = ?updated.next_token_refresh_at,
                "token refresh 成功并推进凭证世代"
            );
            publish_account_if_ready::<P>(state, updated).await
        }
        Err(failure) if failure.kind == MaintenanceFailureKind::Unauthorized => {
            let mut conn = state.db_conn().await?;
            let updated = sql::account::mark_invalid_if_generation(
                &mut conn,
                P::NAME,
                id,
                generation,
                truncate_diagnostic(&failure.message),
            )
            .await?;
            drop(conn);
            if let Some(updated) = updated {
                remove_account_runtime::<P>(state, &updated).await?;
                warn!(
                    provider = P::NAME,
                    provider_account_id = %id,
                    credential_generation = generation,
                    error = %failure,
                    "refresh token 已确认无效，账号进入 invalid 终态"
                );
            }
            Ok(None)
        }
        Err(failure) => {
            // token endpoint 的维护重试与业务请求资源冷却属于不同生命周期。这里仅使用
            // Provider 自己的刷新重试配置，避免调整请求限流策略时意外改变凭证维护节奏。
            let retry_seconds = P::account_refresh_retry_seconds(state).max(1);
            let next_at = Utc::now() + chrono::Duration::seconds(retry_seconds as i64);
            let mut conn = state.db_conn().await?;
            let updated = sql::account::schedule_refresh_retry_if_generation(
                &mut conn,
                P::NAME,
                id,
                generation,
                next_at,
                truncate_diagnostic(&failure.message),
            )
            .await?;
            drop(conn);
            let Some(updated) = updated else {
                info!(
                    provider = P::NAME,
                    provider_account_id = %id,
                    credential_generation = generation,
                    failure_kind = failure.kind.as_str(),
                    "token refresh 失败结果不属于当前凭证世代，已忽略"
                );
                return Ok(None);
            };
            warn!(
                provider = P::NAME,
                provider_account_id = %id,
                credential_generation = generation,
                credential_status = %updated.status,
                failure_kind = failure.kind.as_str(),
                retry_seconds,
                next_token_refresh_at = %next_at,
                error = %failure,
                "token refresh 临时失败，已复用 next refresh 时间安排重试"
            );
            publish_account_if_ready::<P>(state, updated).await
        }
    }
}

async fn run_api_key_maintenance_with_lock<P: MaintenanceProvider>(
    state: &AppState,
    id: Uuid,
    lock: store::LockToken,
) -> AppResult<()> {
    let timeout_seconds = maintenance_job_timeout_seconds(state);
    let result = match timeout(
        Duration::from_secs(timeout_seconds),
        probe_api_key_with_lock::<P>(state, id),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => Err(maintenance_timeout_error::<P>(
            UpstreamResourceKind::ApiKey,
            id,
            timeout_seconds,
        )),
    };
    release_maintenance_lock::<P>(state, UpstreamResourceKind::ApiKey, id, lock).await;
    result
}

async fn probe_api_key_with_lock<P: MaintenanceProvider>(
    state: &AppState,
    id: Uuid,
) -> AppResult<()> {
    let mut conn = state.db_conn().await?;
    let Some(api_key) = sql::api_key::find_by_id(&mut conn, P::NAME, id).await? else {
        store::delete_resource(state, P::NAME, UpstreamResourceKind::ApiKey, id).await?;
        return Ok(());
    };
    drop(conn);
    if !api_key.enabled || api_key.next_probe_at.is_none() {
        reconcile_api_key::<P>(state, api_key).await?;
        return Ok(());
    }
    if api_key.next_probe_at.is_some_and(|next| next > Utc::now()) {
        return Ok(());
    }
    remove_api_key_runtime::<P>(state, &api_key).await?;

    info!(
        provider = P::NAME,
        provider_api_key_id = %id,
        next_probe_at = ?api_key.next_probe_at,
        "maintenance ticker 开始执行官方 API Key 探活"
    );
    match P::probe_api_key(state, &api_key).await {
        Ok(()) => {
            let mut conn = state.db_conn().await?;
            let updated = sql::api_key::record_probe_success(&mut conn, P::NAME, id).await?;
            drop(conn);
            if let Some(updated) = updated {
                let restored = reconcile_api_key::<P>(state, updated).await?;
                info!(
                    provider = P::NAME,
                    provider_api_key_id = %id,
                    runtime_restored = restored,
                    "API Key 探活成功，next_probe_at 与 Error 已清空"
                );
            }
        }
        Err(failure) => {
            let next_at = Utc::now()
                + chrono::Duration::seconds(P::api_key_probe_interval_seconds(state).max(1) as i64);
            let mut conn = state.db_conn().await?;
            let updated = sql::api_key::record_probe_failure(
                &mut conn,
                P::NAME,
                id,
                truncate_diagnostic(&failure.message),
                next_at,
            )
            .await?;
            drop(conn);
            if let Some(updated) = updated {
                remove_api_key_runtime::<P>(state, &updated).await?;
                warn!(
                    provider = P::NAME,
                    provider_api_key_id = %id,
                    next_probe_at = %next_at,
                    error = %failure,
                    "API Key 探活失败，保留单一 Error 并只更新下一次探活时间"
                );
            }
        }
    }
    Ok(())
}

fn maintenance_job_timeout_seconds(state: &AppState) -> u64 {
    state
        .config()
        .provider_upstream_timeout_seconds
        .max(1)
        .saturating_add(MAINTENANCE_JOB_TIMEOUT_GRACE_SECONDS)
}

fn maintenance_lock_ttl_seconds(state: &AppState) -> u64 {
    state
        .config()
        .provider_upstream_timeout_seconds
        .max(1)
        .saturating_add(MAINTENANCE_LOCK_TTL_GRACE_SECONDS)
}

fn maintenance_timeout_error<P: MaintenanceProvider>(
    kind: UpstreamResourceKind,
    id: Uuid,
    timeout_seconds: u64,
) -> AppError {
    AppError::ProviderUpstream {
        provider: P::NAME.to_owned(),
        message: format!(
            "provider maintenance 任务超时: resource_type={}, resource_id={id}, timeout_seconds={timeout_seconds}",
            kind.as_str()
        ),
    }
}

async fn release_maintenance_lock<P: MaintenanceProvider>(
    state: &AppState,
    kind: UpstreamResourceKind,
    id: Uuid,
    lock: store::LockToken,
) {
    if timeout(
        Duration::from_secs(MAINTENANCE_LOCK_RELEASE_TIMEOUT_SECONDS),
        store::release_lock(state, lock),
    )
    .await
    .is_err()
    {
        warn!(
            provider = P::NAME,
            resource_type = kind.as_str(),
            resource_id = %id,
            release_timeout_seconds = MAINTENANCE_LOCK_RELEASE_TIMEOUT_SECONDS,
            "释放 provider maintenance Redis 锁超时，将等待 TTL 自动过期"
        );
    }
}

/// 清理当前快照中已经到期的 quota reset。
///
/// `None` 表示条件更新未命中：账号可能已删除，或 quota 已被并发回执延长。调用方必须
/// 结束本轮 maintenance，不能回退使用传入的旧账号快照；仍到期的其他工作会由下一轮
/// ticker 基于 PostgreSQL 最新事实重新发现。
async fn clear_expired_quota_if_needed<P: MaintenanceProvider>(
    state: &AppState,
    account: ProviderAccount,
) -> AppResult<Option<ProviderAccount>> {
    let now = Utc::now();
    if account
        .quota_resets_at
        .is_none_or(|quota_resets_at| quota_resets_at > now)
    {
        return Ok(Some(account));
    }
    let mut conn = state.db_conn().await?;
    sql::account::clear_quota_resets_at_if_due(&mut conn, P::NAME, account.id, now).await
}

async fn publish_account_if_ready<P: MaintenanceProvider>(
    state: &AppState,
    account: ProviderAccount,
) -> AppResult<Option<UpstreamResource>> {
    if account.group_id.is_none() || !account_is_ready(&account) {
        remove_account_runtime::<P>(state, &account).await?;
        return Ok(None);
    }
    let runtime = resource_from_account::<P>(&account)?;
    if store::publish(state, runtime.clone()).await? {
        Ok(Some(runtime))
    } else {
        Ok(None)
    }
}

fn resource_from_account<P: MaintenanceProvider>(
    account: &ProviderAccount,
) -> AppResult<UpstreamResource> {
    if account.credential_generation <= 0 {
        return Err(AppError::BadRequest {
            message: format!(
                "账号 credential_generation 非法: account_id={}, generation={}",
                account.id, account.credential_generation
            ),
        });
    }
    Ok(UpstreamResource {
        id: account.id,
        provider: account.provider.clone(),
        group_id: account.group_id.ok_or_else(|| AppError::BadRequest {
            message: format!(
                "未分组账号不能构造 Redis runtime: account_id={}",
                account.id
            ),
        })?,
        kind: UpstreamResourceKind::Account,
        auth_secret: account.access_token.trim().to_owned(),
        base_url: None,
        request_context: P::account_request_context(account)?,
        request_override: account.request_override()?,
        credential_generation: Some(account.credential_generation),
        revision: projection_revision(account.updated_at),
    })
}

async fn remove_account_runtime<P: MaintenanceProvider>(
    state: &AppState,
    account: &ProviderAccount,
) -> AppResult<bool> {
    store::remove_at_or_before_revision(
        state,
        P::NAME,
        UpstreamResourceKind::Account,
        account.id,
        projection_revision(account.updated_at),
    )
    .await
}

fn resource_from_api_key<P: MaintenanceProvider>(
    api_key: &ProviderApiKey,
) -> AppResult<UpstreamResource> {
    Ok(UpstreamResource {
        id: api_key.id,
        provider: api_key.provider.clone(),
        group_id: api_key.group_id.ok_or_else(|| AppError::BadRequest {
            message: format!(
                "未分组官方 API Key 不能构造 Redis runtime: api_key_id={}",
                api_key.id
            ),
        })?,
        kind: UpstreamResourceKind::ApiKey,
        auth_secret: api_key.api_key.trim().to_owned(),
        base_url: Some(api_key.base_url.clone()),
        request_context: serde_json::json!({}),
        request_override: api_key.request_override()?,
        credential_generation: None,
        revision: projection_revision(api_key.updated_at),
    })
}

async fn remove_api_key_runtime<P: MaintenanceProvider>(
    state: &AppState,
    api_key: &ProviderApiKey,
) -> AppResult<bool> {
    store::remove_at_or_before_revision(
        state,
        P::NAME,
        UpstreamResourceKind::ApiKey,
        api_key.id,
        projection_revision(api_key.updated_at),
    )
    .await
}

fn account_is_ready(account: &ProviderAccount) -> bool {
    account.enabled && account.status == ACCOUNT_STATUS_VALID && !quota_is_limited(account)
}

fn quota_is_limited(account: &ProviderAccount) -> bool {
    account
        .quota_resets_at
        .is_some_and(|quota_resets_at| quota_resets_at > Utc::now())
}

fn ensure_provider<P: MaintenanceProvider>(provider: &str) -> AppResult<()> {
    if provider == P::NAME {
        return Ok(());
    }
    Err(AppError::BadRequest {
        message: format!(
            "maintenance provider 不匹配: expected={}, actual={provider}",
            P::NAME
        ),
    })
}

fn ensure_kind(runtime: &UpstreamResource, expected: UpstreamResourceKind) -> AppResult<()> {
    if runtime.kind == expected {
        return Ok(());
    }
    Err(AppError::BadRequest {
        message: format!(
            "maintenance 回执资源类型不匹配: expected={}, actual={}",
            expected.as_str(),
            runtime.kind.as_str()
        ),
    })
}

fn truncate_diagnostic(value: &str) -> String {
    const MAX_CHARS: usize = 1024;
    if value.chars().count() <= MAX_CHARS {
        return value.to_owned();
    }
    format!("{}...", value.chars().take(MAX_CHARS).collect::<String>())
}
