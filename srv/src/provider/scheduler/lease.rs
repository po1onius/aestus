//! 上游 attempt 的负载租约。ZSET 同时保存所有权和到期时间，负载直接取有效租约数；
//! 实例启动不修改共享负载，实例退出后遗留占用由过期机制回收。

use std::time::Duration;

use tokio::{
    task::JoinHandle,
    time::{Instant, MissedTickBehavior, interval_at},
};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::{
    err::{AppError, AppResult},
    infra::redis::RedisConnection,
    state::AppState,
};

use super::UpstreamAllocation;

const LEASE_TTL: Duration = Duration::from_secs(90);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
const KEY_TTL: Duration = Duration::from_secs(120);

// 时间统一取自 Redis，避免不同网关实例的时钟差异改变租约有效性。
const ACQUIRE_LUA: &str = r#"
local time = redis.call('TIME')
local now = tonumber(time[1]) * 1000 + math.floor(tonumber(time[2]) / 1000)
local reclaimed = redis.call('ZREMRANGEBYSCORE', KEYS[1], '-inf', now)
local added = redis.call('ZADD', KEYS[1], 'NX', now + tonumber(ARGV[2]), ARGV[1])
if added == 1 then
    redis.call('PEXPIRE', KEYS[1], ARGV[3])
end
return {added, redis.call('ZCARD', KEYS[1]), reclaimed}
"#;

// 只更新仍有效的 token。迟到的续租不能复活已经释放或到期的 attempt。
const RENEW_LUA: &str = r#"
local time = redis.call('TIME')
local now = tonumber(time[1]) * 1000 + math.floor(tonumber(time[2]) / 1000)
local reclaimed = redis.call('ZREMRANGEBYSCORE', KEYS[1], '-inf', now)
if not redis.call('ZSCORE', KEYS[1], ARGV[1]) then
    return {0, redis.call('ZCARD', KEYS[1]), reclaimed}
end
redis.call('ZADD', KEYS[1], 'XX', now + tonumber(ARGV[2]), ARGV[1])
redis.call('PEXPIRE', KEYS[1], ARGV[3])
return {1, redis.call('ZCARD', KEYS[1]), reclaimed}
"#;

const RELEASE_LUA: &str = r#"
local time = redis.call('TIME')
local now = tonumber(time[1]) * 1000 + math.floor(tonumber(time[2]) / 1000)
redis.call('ZREMRANGEBYSCORE', KEYS[1], '-inf', now)
local removed = redis.call('ZREM', KEYS[1], ARGV[1])
return {removed, redis.call('ZCARD', KEYS[1])}
"#;

const READ_LOAD_LUA: &str = r#"
local time = redis.call('TIME')
local now = tonumber(time[1]) * 1000 + math.floor(tonumber(time[2]) / 1000)
local reclaimed = redis.call('ZREMRANGEBYSCORE', KEYS[1], '-inf', now)
return {redis.call('ZCARD', KEYS[1]), reclaimed}
"#;

/// 后台续租只携带身份，不复制 runtime 中的上游凭证或整个 allocation。
#[derive(Clone)]
struct LeaseIdentity {
    request_id: Uuid,
    lease_id: Uuid,
    provider: String,
    member: String,
    key: String,
}

/// 由 proxy 持有到本次 attempt 完成，涵盖上游构造、发送和完整响应生命周期。
/// 显式释放、取消和 panic 都会停止 heartbeat；进程被终止时由 Redis 到期回收。
pub struct UpstreamLease {
    state: AppState,
    identity: LeaseIdentity,
    allocation: Option<UpstreamAllocation>,
    heartbeat: Option<JoinHandle<()>>,
}

impl UpstreamLease {
    pub(super) async fn acquire(
        state: AppState,
        allocation: UpstreamAllocation,
    ) -> AppResult<(Self, i64)> {
        let member = allocation.resource_member();
        let identity = LeaseIdentity {
            request_id: allocation.request_id,
            lease_id: Uuid::now_v7(),
            provider: allocation.resource.provider.clone(),
            key: lease_key(&allocation.resource.provider, &member),
            member,
        };
        // 在首次 Redis await 前建立 guard，使登记过程中被取消也能释放相同 token。
        let mut lease = Self {
            state,
            identity,
            allocation: Some(allocation),
            heartbeat: None,
        };
        let mut redis = lease.state.redis();
        let (added, current, reclaimed): (i64, i64, i64) = redis::cmd("EVAL")
            .arg(ACQUIRE_LUA)
            .arg(1)
            .arg(&lease.identity.key)
            .arg(lease.identity.lease_id.to_string())
            .arg(millis(LEASE_TTL))
            .arg(millis(KEY_TTL))
            .query_async(&mut redis)
            .await
            .map_err(super::redis_error)?;
        if added != 1 {
            // UUID 冲突时不拥有该 token，不能让 Drop 删除另一个请求的租约。
            lease.allocation.take();
            return Err(AppError::Redis {
                message: format!("上游资源 lease token 冲突: {}", lease.id()),
            });
        }
        lease.start_heartbeat();
        info!(
            request_id = %lease.identity.request_id, lease_id = %lease.id(),
            provider = %lease.identity.provider, resource_member = %lease.identity.member,
            inflight_count = current, reclaimed_expired_leases = reclaimed,
            lease_ttl_seconds = LEASE_TTL.as_secs(), heartbeat_seconds = HEARTBEAT_INTERVAL.as_secs(),
            "上游资源负载租约已登记"
        );
        Ok((lease, current))
    }

    pub(super) fn id(&self) -> Uuid {
        self.identity.lease_id
    }

    pub fn allocation(&self) -> &UpstreamAllocation {
        self.allocation
            .as_ref()
            .expect("尚未释放的 UpstreamLease 必须持有 allocation")
    }

    fn start_heartbeat(&mut self) {
        let state = self.state.clone();
        let identity = self.identity.clone();
        self.heartbeat = Some(tokio::spawn(async move {
            let mut ticker = interval_at(Instant::now() + HEARTBEAT_INTERVAL, HEARTBEAT_INTERVAL);
            ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
            loop {
                ticker.tick().await;
                match renew(&state, &identity).await {
                    Ok(true) => {}
                    Ok(false) => {
                        warn!(request_id = %identity.request_id, lease_id = %identity.lease_id,
                            provider = %identity.provider, resource_member = %identity.member,
                            "上游负载租约已不存在，停止续租；不重新登记已经到期的 token");
                        break;
                    }
                    Err(error) => {
                        error!(request_id = %identity.request_id, lease_id = %identity.lease_id,
                        provider = %identity.provider, resource_member = %identity.member, error = %error,
                        "上游负载租约续租失败，等待下一续租周期")
                    }
                }
            }
        }));
    }

    fn stop_heartbeat(&mut self) {
        if let Some(task) = self.heartbeat.take() {
            task.abort();
        }
    }

    pub async fn release(mut self) -> AppResult<()> {
        self.stop_heartbeat();
        let result = release_identity(&self.state, &self.identity).await;
        if result.is_ok() {
            self.allocation.take();
        }
        result
    }

    /// Stream::poll_next / Drop 不能 await：当场停止续租，独立提交 Redis 释放。
    /// 后台任务只负责负载，不持有或等待插件、维护回执。
    pub fn release_in_background(mut self, reason: &'static str) {
        self.stop_heartbeat();
        info!(request_id = %self.identity.request_id, lease_id = %self.identity.lease_id,
            provider = %self.identity.provider, resource_member = %self.identity.member,
            reason, "上游请求已结束，停止续租并提交负载释放");
        tokio::spawn(async move {
            if let Err(error) = self.release().await {
                error!(reason, error = %error,
                    "上游请求结束后的负载释放失败，Drop 已提交幂等释放");
            }
        });
    }
}

impl Drop for UpstreamLease {
    fn drop(&mut self) {
        self.stop_heartbeat();
        if self.allocation.take().is_none() {
            return;
        }
        let state = self.state.clone();
        let identity = self.identity.clone();
        warn!(request_id = %identity.request_id, lease_id = %identity.lease_id,
            provider = %identity.provider, resource_member = %identity.member,
            "上游负载租约在显式释放前结束，已停止续租并提交后台释放");
        tokio::spawn(async move {
            if let Err(error) = release_identity(&state, &identity).await {
                error!(request_id = %identity.request_id, lease_id = %identity.lease_id,
                    provider = %identity.provider, resource_member = %identity.member, error = %error,
                    "后台释放上游负载租约失败，遗留记录将在 TTL 后失效");
            }
        });
    }
}

async fn renew(state: &AppState, identity: &LeaseIdentity) -> AppResult<bool> {
    let mut redis = state.redis();
    let (renewed, current, reclaimed): (i64, i64, i64) = redis::cmd("EVAL")
        .arg(RENEW_LUA)
        .arg(1)
        .arg(&identity.key)
        .arg(identity.lease_id.to_string())
        .arg(millis(LEASE_TTL))
        .arg(millis(KEY_TTL))
        .query_async(&mut redis)
        .await
        .map_err(super::redis_error)?;
    debug!(request_id = %identity.request_id, lease_id = %identity.lease_id,
        provider = %identity.provider, resource_member = %identity.member,
        renewed, inflight_count = current, reclaimed_expired_leases = reclaimed,
        "上游负载租约续租完成");
    Ok(renewed == 1)
}

async fn release_identity(state: &AppState, identity: &LeaseIdentity) -> AppResult<()> {
    let mut redis = state.redis();
    let (released, current): (i64, i64) = redis::cmd("EVAL")
        .arg(RELEASE_LUA)
        .arg(1)
        .arg(&identity.key)
        .arg(identity.lease_id.to_string())
        .query_async(&mut redis)
        .await
        .map_err(super::redis_error)?;
    info!(request_id = %identity.request_id, lease_id = %identity.lease_id,
        provider = %identity.provider, resource_member = %identity.member,
        release_applied = released == 1, inflight_count = current,
        "上游资源负载租约已释放");
    Ok(())
}

/// 调度和 Dashboard 共用同一负载计算，逐资源原子回收后计数；pipeline 合并网络往返。
pub(super) async fn read_loads(
    redis: &mut RedisConnection,
    provider: &str,
    members: &[String],
) -> AppResult<Vec<i64>> {
    if members.is_empty() {
        return Ok(Vec::new());
    }
    let mut pipe = redis::pipe();
    for member in members {
        pipe.cmd("EVAL")
            .arg(READ_LOAD_LUA)
            .arg(1)
            .arg(lease_key(provider, member));
    }
    let results: Vec<(i64, i64)> = pipe.query_async(redis).await.map_err(super::redis_error)?;
    if results.len() != members.len() {
        return Err(AppError::Redis {
            message: "读取上游资源负载租约数量不匹配".to_owned(),
        });
    }
    let reclaimed: i64 = results.iter().map(|(_, reclaimed)| reclaimed).sum();
    if reclaimed > 0 {
        info!(
            provider,
            resource_count = members.len(),
            reclaimed_expired_leases = reclaimed,
            "批量读取上游负载时已回收过期租约"
        );
    }
    Ok(results.into_iter().map(|(current, _)| current).collect())
}

fn lease_key(provider: &str, member: &str) -> String {
    format!("provider:{provider}:resource:{member}:leases")
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).expect("上游负载租约 TTL 必须适合 u64")
}
