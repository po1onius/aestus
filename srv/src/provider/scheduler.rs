use std::collections::{HashMap, HashSet};

use redis::AsyncCommands;
use sha2::{Digest, Sha256};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::{
    err::{AppError, AppResult},
    provider::{resource::UpstreamResource, runtime::store},
    state::AppState,
};

const INFLIGHT_SCORE_WEIGHT: i64 = 1_000;
const STICKY_SCORE_BONUS: i64 = 500;

const RELEASE_LOAD_LUA: &str = r#"
local current = tonumber(redis.call('HGET', KEYS[1], ARGV[1]) or '0')
if current <= 1 then
    redis.call('HSET', KEYS[1], ARGV[1], 0)
    return 0
end
return redis.call('HINCRBY', KEYS[1], ARGV[1], -1)
"#;

#[derive(Debug, Clone)]
pub struct UpstreamAllocation {
    /// 网关入口创建的请求生命周期 ID；同一请求的全部重试共用该值。
    pub request_id: Uuid,
    pub resource: UpstreamResource,
}

impl UpstreamAllocation {
    pub fn resource_member(&self) -> String {
        self.resource.resource_member()
    }

    pub fn resource_type(&self) -> &'static str {
        self.resource.kind.as_str()
    }
}

/// 一次上游资源占用的 RAII lease。
///
/// 正常路径通过 `release` 保持原有同步释放和错误传播语义；如果请求 future 在释放前被
/// 取消或 panic，Drop 会在后台补发同一个幂等释放操作，避免 inflight 计数长期泄漏。
pub struct UpstreamLease {
    state: AppState,
    allocation: Option<UpstreamAllocation>,
}

impl UpstreamLease {
    fn new(state: AppState, allocation: UpstreamAllocation) -> Self {
        Self {
            state,
            allocation: Some(allocation),
        }
    }

    pub fn allocation(&self) -> &UpstreamAllocation {
        self.allocation
            .as_ref()
            .expect("尚未释放的 UpstreamLease 必须持有 allocation")
    }

    pub async fn release(mut self) -> AppResult<()> {
        let result = release_allocation(&self.state, self.allocation()).await;
        if result.is_ok() {
            self.allocation.take();
        }
        result
    }
}

impl Drop for UpstreamLease {
    fn drop(&mut self) {
        let Some(allocation) = self.allocation.take() else {
            return;
        };
        let state = self.state.clone();
        warn!(
            request_id = %allocation.request_id,
            provider = %allocation.resource.provider,
            resource_type = allocation.resource_type(),
            resource_id = %allocation.resource.id,
            "上游资源 lease 在显式释放前结束，RAII guard 已提交兜底释放"
        );
        tokio::spawn(async move {
            if let Err(error) = release_allocation(&state, &allocation).await {
                error!(
                    request_id = %allocation.request_id,
                    provider = %allocation.resource.provider,
                    resource_type = allocation.resource_type(),
                    resource_id = %allocation.resource.id,
                    error = %error,
                    "RAII guard 兜底释放上游资源失败"
                );
            }
        });
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ResourceLoadView {
    pub inflight_count: i64,
}

#[derive(Debug)]
struct Candidate {
    resource: UpstreamResource,
    inflight_count: i64,
    score: i64,
    sticky_hit: bool,
}

/// 从 provider 的统一 runtime index 选择上游资源。
///
/// 请求级排除集合由通用 executor 持有并随每次 acquire 显式传入，scheduler 不保存跨请求
/// 可变状态。它与 provider 回执形成两层边界：排除集合保证当前请求不会重试集合中的资源，
/// maintenance runtime 则表达资源对所有请求的持久可用性。token refresh 与 API Key probe
/// 仍只由 maintenance ticker 触发。`request_id` 由 gateway 创建并贯穿全部 attempt。
pub async fn acquire(
    state: &AppState,
    request_id: Uuid,
    provider: &str,
    group_id: Uuid,
    sticky_session_key: Option<&str>,
    excluded_resource_members: &HashSet<String>,
) -> AppResult<UpstreamLease> {
    let mut redis = state.redis();
    let sticky_hash = sticky_session_key.map(sticky_hash);
    // sticky 候选固定放在首位，使同分情况下继续保持原有的 sticky 优先语义。候选 payload
    // 由 runtime store 批量读取，下面再用一次 HMGET 取得全部 inflight。
    let mut candidate_resources = Vec::<(String, UpstreamResource, bool)>::new();

    if let Some(sticky_hash) = sticky_hash.as_deref()
        && let Some(member) = redis
            .get::<_, Option<String>>(sticky_key(provider, group_id, sticky_hash))
            .await
            .map_err(redis_error)?
    {
        if excluded_resource_members.contains(&member) {
            debug!(
                request_id = %request_id,
                provider,
                provider_group_id = %group_id,
                runtime_member = %member,
                excluded_resource_count = excluded_resource_members.len(),
                "scheduler sticky 资源已被当前请求排除"
            );
        } else if let Some(resource) =
            store::read_indexed(&mut redis, provider, group_id, &member).await?
        {
            candidate_resources.push((member, resource, true));
        }
    }

    // 抽样窗口至少比排除集合多一个成员。即使部署方把候选上限配置为 1，下一次 attempt
    // 也能越过已经失败的资源取得一个新候选，而不是因抽样恰好命中排除项而误报资源耗尽。
    let minimum_candidate_limit = excluded_resource_members.len().saturating_add(1);
    let resources = store::load_resources(
        state,
        &mut redis,
        provider,
        group_id,
        minimum_candidate_limit,
    )
    .await?;
    for resource in resources {
        let member = resource.resource_member();
        if excluded_resource_members.contains(&member) {
            continue;
        }
        if candidate_resources
            .iter()
            .any(|(candidate_member, _, _)| candidate_member == &member)
        {
            continue;
        }
        candidate_resources.push((member, resource, false));
    }

    let candidate_members = candidate_resources
        .iter()
        .map(|(member, _, _)| member.clone())
        .collect::<Vec<_>>();
    let inflight_counts = read_loads(&mut redis, provider, &candidate_members).await?;
    debug!(
        request_id = %request_id,
        provider,
        provider_group_id = %group_id,
        candidate_count = candidate_resources.len(),
        excluded_resource_count = excluded_resource_members.len(),
        sticky_candidate = candidate_resources
            .first()
            .is_some_and(|(_, _, sticky_hit)| *sticky_hit),
        "scheduler 候选 inflight 已通过单次 HMGET 批量读取"
    );

    let mut selected = None;
    for ((_, resource, sticky_hit), inflight_count) in
        candidate_resources.into_iter().zip(inflight_counts)
    {
        let candidate = Candidate {
            resource,
            inflight_count,
            score: if sticky_hit {
                score(inflight_count).saturating_sub(STICKY_SCORE_BONUS)
            } else {
                score(inflight_count)
            },
            sticky_hit,
        };
        if selected
            .as_ref()
            .is_none_or(|selected: &Candidate| candidate.score < selected.score)
        {
            selected = Some(candidate);
        }
    }

    let selected = selected.ok_or_else(|| AppError::ResourceError {
        provider: provider.to_owned(),
        group_id,
        message: if excluded_resource_members.is_empty() {
            "maintenance 可用集合中没有候选资源".to_owned()
        } else {
            format!(
                "排除当前请求已尝试的 {} 个资源后没有候选资源",
                excluded_resource_members.len()
            )
        },
    })?;
    let allocation = UpstreamAllocation {
        request_id,
        resource: selected.resource,
    };
    let member = allocation.resource_member();
    let new_inflight_count: i64 = redis
        .hincr(load_key(provider), &member, 1)
        .await
        .map_err(redis_error)?;
    // 从 inflight 增加成功这一刻开始立即交给 lease 持有。后续 sticky 写入或日志路径若被
    // 取消，Drop 也能回收本次占用，不留下 acquire 内部的时间窗口。
    let lease = UpstreamLease::new(state.clone(), allocation);

    if let Some(sticky_hash) = sticky_hash.as_deref() {
        let ttl = state.config().provider_session_sticky_ttl_seconds.max(1);
        let _: () = redis
            .set_ex(sticky_key(provider, group_id, sticky_hash), &member, ttl)
            .await
            .map_err(redis_error)?;
    }

    let allocation = lease.allocation();
    info!(
        request_id = %allocation.request_id,
        provider,
        provider_group_id = %group_id,
        resource_type = allocation.resource_type(),
        resource_id = %allocation.resource.id,
        runtime_revision = allocation.resource.revision,
        sticky_hit = selected.sticky_hit,
        previous_inflight_count = selected.inflight_count,
        new_inflight_count,
        scheduler_score = selected.score,
        "provider 上游资源调度成功"
    );
    Ok(lease)
}

async fn release_allocation(state: &AppState, allocation: &UpstreamAllocation) -> AppResult<()> {
    let mut redis = state.redis();
    let provider = &allocation.resource.provider;
    let member = allocation.resource_member();
    let inflight_count: i64 = redis::cmd("EVAL")
        .arg(RELEASE_LOAD_LUA)
        .arg(1)
        .arg(load_key(provider))
        .arg(&member)
        .query_async(&mut redis)
        .await
        .map_err(redis_error)?;
    info!(
        request_id = %allocation.request_id,
        provider,
        resource_type = allocation.resource_type(),
        resource_id = %allocation.resource.id,
        inflight_count,
        "provider 上游资源占用已释放"
    );
    Ok(())
}

pub(super) async fn reset_loads(state: &AppState, provider: &str) -> AppResult<()> {
    let mut redis = state.redis();
    let _: usize = redis.del(load_key(provider)).await.map_err(redis_error)?;
    info!(provider, "provider scheduler 遗留负载已清理");
    Ok(())
}

pub async fn load_views(
    state: &AppState,
    provider: &str,
    members: &[String],
) -> AppResult<HashMap<String, ResourceLoadView>> {
    if members.is_empty() {
        return Ok(HashMap::new());
    }
    let mut redis = state.redis();
    let counts = read_loads(&mut redis, provider, members).await?;
    Ok(members
        .iter()
        .cloned()
        .zip(
            counts
                .into_iter()
                .map(|inflight_count| ResourceLoadView { inflight_count }),
        )
        .collect())
}

/// 按资源类型批量读取管理端需要的负载视图，并把 Redis member 映射回持久实体 ID。
pub async fn load_kind_views(
    state: &AppState,
    provider: &str,
    kind: crate::provider::resource::UpstreamResourceKind,
    ids: &[Uuid],
) -> AppResult<HashMap<Uuid, ResourceLoadView>> {
    let members = ids
        .iter()
        .map(|id| format!("{}:{id}", kind.as_str()))
        .collect::<Vec<_>>();
    let mut views = load_views(state, provider, &members).await?;
    Ok(ids
        .iter()
        .copied()
        .zip(members)
        .filter_map(|(id, member)| views.remove(&member).map(|view| (id, view)))
        .collect())
}

async fn read_loads(
    redis: &mut crate::infra::redis::RedisConnection,
    provider: &str,
    members: &[String],
) -> AppResult<Vec<i64>> {
    if members.is_empty() {
        return Ok(Vec::new());
    }
    let counts: Vec<Option<i64>> = redis::cmd("HMGET")
        .arg(load_key(provider))
        .arg(members)
        .query_async(&mut *redis)
        .await
        .map_err(redis_error)?;
    if counts.len() != members.len() {
        return Err(AppError::Redis {
            message: "读取 provider 上游资源负载数量不匹配".to_owned(),
        });
    }
    Ok(counts
        .into_iter()
        .map(|count| count.unwrap_or_default().max(0))
        .collect())
}

fn score(inflight_count: i64) -> i64 {
    inflight_count.max(0).saturating_mul(INFLIGHT_SCORE_WEIGHT)
}

fn load_key(provider: &str) -> String {
    format!("provider:{provider}:resource:load")
}

fn sticky_key(provider: &str, group_id: Uuid, hash: &str) -> String {
    format!("provider:{provider}:group:{group_id}:session:sticky:{hash}")
}

fn sticky_hash(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

fn redis_error(source: redis::RedisError) -> AppError {
    AppError::Redis {
        message: source.to_string(),
    }
}
