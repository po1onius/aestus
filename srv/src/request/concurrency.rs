//! 用户级网关请求并发租约。
//!
//! PostgreSQL 中的最大并发配置是持久真源；本模块只在 Redis 中维护正在执行的请求。
//! 每个租约按 tenant、user、provider 隔离存入 ZSET，member 是单次 acquire 生成的唯一
//! token，score 是 Redis 服务端时间计算出的过期时间。即使用户当前没有配置上限也会登记
//! 租约，因此从“不限”调整为有限值后，已经在途的请求仍会被计入。

use std::{collections::HashMap, time::Duration};

use axum::{body::Body, http::Response};
use futures_util::{StreamExt, stream};
use tokio::{
    sync::oneshot,
    task::JoinHandle,
    time::{Instant, MissedTickBehavior},
};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::{
    err::{AppError, AppResult},
    state::AppState,
};

/// 正常情况下 heartbeat 至少有两次机会在租约过期前完成续租。
const LEASE_TTL: Duration = Duration::from_secs(90);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
/// ZSET key 比 member lease 多存活一个 heartbeat 周期，避免过期边界处 key 先消失。
const KEY_TTL: Duration = Duration::from_secs(120);

/// 清理过期 member、检查限制并原子登记新 lease。
///
/// 返回值依次是：是否获准、清理后的并发数、清理掉的过期 lease 数。token 已经存在时
/// 视为同一次命令的幂等重放，并刷新其过期时间。
const ACQUIRE_LUA: &str = r#"
local redis_time = redis.call('TIME')
local now_ms = tonumber(redis_time[1]) * 1000 + math.floor(tonumber(redis_time[2]) / 1000)
local expires_at_ms = now_ms + tonumber(ARGV[2])
local reclaimed = redis.call('ZREMRANGEBYSCORE', KEYS[1], '-inf', now_ms)

if redis.call('ZSCORE', KEYS[1], ARGV[1]) then
    redis.call('ZADD', KEYS[1], expires_at_ms, ARGV[1])
    redis.call('PEXPIRE', KEYS[1], ARGV[4])
    return {1, redis.call('ZCARD', KEYS[1]), reclaimed}
end

local current = redis.call('ZCARD', KEYS[1])
local limit = tonumber(ARGV[3])
if limit >= 0 and current >= limit then
    return {0, current, reclaimed}
end

redis.call('ZADD', KEYS[1], expires_at_ms, ARGV[1])
redis.call('PEXPIRE', KEYS[1], ARGV[4])
return {1, current + 1, reclaimed}
"#;

/// 只有 token 仍存在时才续租，保证释放与 heartbeat 并发时不会重新创建 lease。
/// 返回值依次是：是否续租、当前并发数、顺便清理掉的过期 lease 数。
const HEARTBEAT_LUA: &str = r#"
local redis_time = redis.call('TIME')
local now_ms = tonumber(redis_time[1]) * 1000 + math.floor(tonumber(redis_time[2]) / 1000)
local expires_at_ms = now_ms + tonumber(ARGV[2])
local reclaimed = redis.call('ZREMRANGEBYSCORE', KEYS[1], '-inf', now_ms)

if not redis.call('ZSCORE', KEYS[1], ARGV[1]) then
    return {0, redis.call('ZCARD', KEYS[1]), reclaimed}
end

redis.call('ZADD', KEYS[1], expires_at_ms, ARGV[1])
redis.call('PEXPIRE', KEYS[1], ARGV[3])
return {1, redis.call('ZCARD', KEYS[1]), reclaimed}
"#;

/// 按 token 幂等释放；空 ZSET 会一并删除。
/// 返回值依次是：本次是否实际删除、释放后的并发数。
const RELEASE_LUA: &str = r#"
local removed = redis.call('ZREM', KEYS[1], ARGV[1])
local current = redis.call('ZCARD', KEYS[1])
if current == 0 then
    redis.call('DEL', KEYS[1])
end
return {removed, current}
"#;

/// 使用 Redis 服务端时间清理单个 ZSET 的过期租约并返回实时并发数。
///
/// 返回值依次是：当前并发数、清理掉的过期 lease 数。空 ZSET 会一并删除，避免
/// Dashboard 的只读查询留下无意义的空 key。
const ACTIVE_COUNT_LUA: &str = r#"
local redis_time = redis.call('TIME')
local now_ms = tonumber(redis_time[1]) * 1000 + math.floor(tonumber(redis_time[2]) / 1000)
local reclaimed = redis.call('ZREMRANGEBYSCORE', KEYS[1], '-inf', now_ms)
local current = redis.call('ZCARD', KEYS[1])
if current == 0 then
    redis.call('DEL', KEYS[1])
end
return {current, reclaimed}
"#;

/// Dashboard 批量查询得到的用户 × provider 实时并发映射。
///
/// 未出现的用户或 provider 明确按 0 处理；调用方无需了解 Redis key 结构，也不会依赖
/// pipeline 响应的隐式顺序。
#[derive(Debug, Default)]
pub struct ActiveConcurrencyByUser {
    counts: HashMap<Uuid, HashMap<String, u32>>,
}

impl ActiveConcurrencyByUser {
    pub fn count(&self, user_id: Uuid, provider: &str) -> u32 {
        self.counts
            .get(&user_id)
            .and_then(|provider_counts| provider_counts.get(provider))
            .copied()
            .unwrap_or_default()
    }
}

/// 用户并发准入结果。
pub enum AcquireResult {
    /// 已经在 Redis 登记；调用方必须持有 lease 到请求生命周期结束。
    Acquired(UserConcurrencyLease),
    /// 当前 provider 下该用户已经达到配置上限。
    LimitExceeded { current: u32, limit: u32 },
}

#[derive(Clone)]
struct LeaseIdentity {
    request_id: Uuid,
    lease_id: Uuid,
    tenant_id: String,
    user_id: Uuid,
    provider: &'static str,
    redis_key: String,
}

/// 一个已登记的用户并发槽位。
///
/// 普通响应就绪后由 gateway 显式释放；流式响应通过 [`hold_streaming_response`] 托管。
/// 若请求或流被取消、发生 panic，`Drop` 停止续租并提交一次幂等后台释放。
pub struct UserConcurrencyLease {
    state: AppState,
    identity: Option<LeaseIdentity>,
    heartbeat_stop: Option<oneshot::Sender<()>>,
    heartbeat_task: Option<JoinHandle<()>>,
}

impl UserConcurrencyLease {
    fn pending(state: AppState, identity: LeaseIdentity) -> Self {
        Self {
            state,
            identity: Some(identity),
            heartbeat_stop: None,
            heartbeat_task: None,
        }
    }

    fn start_heartbeat(&mut self) {
        let identity = self
            .identity
            .as_ref()
            .expect("已获准的用户并发 lease 必须持有 identity")
            .clone();
        let state = self.state.clone();
        let (stop_tx, mut stop_rx) = oneshot::channel();
        let task = tokio::spawn(async move {
            let mut interval =
                tokio::time::interval_at(Instant::now() + HEARTBEAT_INTERVAL, HEARTBEAT_INTERVAL);
            interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

            loop {
                tokio::select! {
                    _ = &mut stop_rx => {
                        debug!(
                            request_id = %identity.request_id,
                            lease_id = %identity.lease_id,
                            tenant_id = %identity.tenant_id,
                            user_id = %identity.user_id,
                            provider = identity.provider,
                            "用户并发 lease heartbeat 已停止"
                        );
                        return;
                    }
                    _ = interval.tick() => {
                        match renew(&state, &identity).await {
                            Ok(true) => {}
                            Ok(false) => {
                                warn!(
                                    request_id = %identity.request_id,
                                    lease_id = %identity.lease_id,
                                    tenant_id = %identity.tenant_id,
                                    user_id = %identity.user_id,
                                    provider = identity.provider,
                                    "用户并发 lease 在 heartbeat 前已不存在，续租任务结束"
                                );
                                return;
                            }
                            Err(error) => {
                                error!(
                                    request_id = %identity.request_id,
                                    lease_id = %identity.lease_id,
                                    tenant_id = %identity.tenant_id,
                                    user_id = %identity.user_id,
                                    provider = identity.provider,
                                    error = %error,
                                    "用户并发 lease heartbeat 失败，将在下一周期继续续租"
                                );
                            }
                        }
                    }
                }
            }
        });
        self.heartbeat_stop = Some(stop_tx);
        self.heartbeat_task = Some(task);
    }

    fn stop_heartbeat(&mut self) {
        if let Some(stop) = self.heartbeat_stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.heartbeat_task.take() {
            task.abort();
        }
    }

    fn disarm(&mut self) {
        self.stop_heartbeat();
        self.identity.take();
    }

    /// 显式释放并等待 Redis 确认。释放失败时 lease 保持 armed，函数结束时的 `Drop`
    /// 会立即提交同一 token 的幂等兜底释放。
    pub async fn release(mut self) -> AppResult<()> {
        self.stop_heartbeat();
        let identity = self
            .identity
            .as_ref()
            .expect("尚未释放的用户并发 lease 必须持有 identity");
        let result = release_identity(&self.state, identity).await;
        match &result {
            Ok(()) => {
                self.identity.take();
            }
            Err(error) => {
                error!(
                    request_id = %identity.request_id,
                    lease_id = %identity.lease_id,
                    tenant_id = %identity.tenant_id,
                    user_id = %identity.user_id,
                    provider = identity.provider,
                    error = %error,
                    "显式释放用户并发 lease 失败，将由 RAII guard 提交兜底释放"
                );
            }
        }
        result
    }
}

impl Drop for UserConcurrencyLease {
    fn drop(&mut self) {
        self.stop_heartbeat();
        let Some(identity) = self.identity.take() else {
            return;
        };
        let state = self.state.clone();
        warn!(
            request_id = %identity.request_id,
            lease_id = %identity.lease_id,
            tenant_id = %identity.tenant_id,
            user_id = %identity.user_id,
            provider = identity.provider,
            "用户请求或响应流在显式释放前结束，已停止续租并提交后台释放"
        );
        tokio::spawn(async move {
            if let Err(error) = release_identity(&state, &identity).await {
                error!(
                    request_id = %identity.request_id,
                    lease_id = %identity.lease_id,
                    tenant_id = %identity.tenant_id,
                    user_id = %identity.user_id,
                    provider = identity.provider,
                    error = %error,
                    "RAII guard 兜底释放用户并发 lease 失败"
                );
            }
        });
    }
}

/// 原子申请用户在指定 provider 下的一个并发槽位。
///
/// `None` 表示当前不限流，但请求仍会写入 ZSET。Redis 命令执行失败直接返回基础设施错误，
/// 不在业务层增加重试或本地计数兜底。
pub async fn acquire(
    state: &AppState,
    request_id: Uuid,
    tenant_id: String,
    user_id: Uuid,
    provider: &'static str,
    max_concurrency: Option<i32>,
) -> AppResult<AcquireResult> {
    let identity = LeaseIdentity {
        request_id,
        lease_id: Uuid::now_v7(),
        tenant_id: tenant_id.clone(),
        user_id,
        provider,
        redis_key: lease_key(&tenant_id, user_id, provider),
    };
    // guard 在 Redis await 前就进入 armed 状态。即使命令已经执行、但等待响应的 future
    // 随后被取消，Drop 仍会用相同 token 补发幂等释放。
    let mut lease = UserConcurrencyLease::pending(state.clone(), identity.clone());
    let mut redis = state.redis();
    let result: Vec<i64> = redis::cmd("EVAL")
        .arg(ACQUIRE_LUA)
        .arg(1)
        .arg(&identity.redis_key)
        .arg(identity.lease_id.to_string())
        .arg(duration_millis(LEASE_TTL))
        .arg(max_concurrency.map(i64::from).unwrap_or(-1))
        .arg(duration_millis(KEY_TTL))
        .query_async(&mut redis)
        .await
        .map_err(redis_error)?;
    let [acquired, current, reclaimed] = result.as_slice() else {
        return Err(AppError::Redis {
            message: "登记用户并发 lease 时响应格式无效".to_owned(),
        });
    };
    let current = count_from_redis(*current)?;

    if *acquired == 0 {
        lease.disarm();
        let limit = max_concurrency
            .and_then(|value| u32::try_from(value).ok())
            .unwrap_or(0);
        info!(
            request_id = %request_id,
            tenant_id = %tenant_id,
            user_id = %user_id,
            provider,
            current,
            limit,
            reclaimed_expired_leases = *reclaimed,
            "用户请求达到 provider 并发上限，准入已拒绝"
        );
        return Ok(AcquireResult::LimitExceeded { current, limit });
    }
    if *acquired != 1 {
        return Err(AppError::Redis {
            message: format!("登记用户并发 lease 时返回未知状态: {acquired}"),
        });
    }

    lease.start_heartbeat();
    info!(
        request_id = %request_id,
        lease_id = %identity.lease_id,
        tenant_id = %tenant_id,
        user_id = %user_id,
        provider,
        current,
        max_concurrency,
        reclaimed_expired_leases = *reclaimed,
        lease_ttl_seconds = LEASE_TTL.as_secs(),
        heartbeat_interval_seconds = HEARTBEAT_INTERVAL.as_secs(),
        "用户 provider 并发 lease 获取成功"
    );
    Ok(AcquireResult::Acquired(lease))
}

/// 批量读取一组用户在各 provider 下的实时并发数。
///
/// 所有 key 通过一个 Redis pipeline 往返读取；每个 key 独立执行 Lua，使用 Redis
/// 服务端时间先回收过期 lease，再读取 ZSET 数量。Redis 读取失败直接返回基础设施错误，
/// 不用不准确的本地值降级 Dashboard 展示。
pub async fn active_counts_for_users(
    state: &AppState,
    tenant_id: String,
    user_ids: &[Uuid],
    providers: &[&str],
) -> AppResult<ActiveConcurrencyByUser> {
    let mut active_counts = ActiveConcurrencyByUser {
        counts: user_ids
            .iter()
            .copied()
            .map(|user_id| (user_id, HashMap::with_capacity(providers.len())))
            .collect(),
    };
    let tenant_key_prefix = tenant_id.as_str();
    let requested_keys = user_ids
        .iter()
        .flat_map(|user_id| {
            providers.iter().map(move |provider| {
                (
                    *user_id,
                    *provider,
                    lease_key(tenant_key_prefix, *user_id, provider),
                )
            })
        })
        .collect::<Vec<_>>();

    if requested_keys.is_empty() {
        debug!(
            tenant_id = %tenant_id,
            user_count = user_ids.len(),
            provider_count = providers.len(),
            "批量读取用户实时并发时没有需要查询的 Redis key"
        );
        return Ok(active_counts);
    }

    let mut redis = state.redis();
    let mut pipe = redis::pipe();
    for (_, _, redis_key) in &requested_keys {
        pipe.cmd("EVAL").arg(ACTIVE_COUNT_LUA).arg(1).arg(redis_key);
    }
    let results: Vec<Vec<i64>> = pipe.query_async(&mut redis).await.map_err(redis_error)?;
    if results.len() != requested_keys.len() {
        return Err(AppError::Redis {
            message: format!(
                "批量读取用户实时并发时响应数量不匹配: expected={}, actual={}",
                requested_keys.len(),
                results.len()
            ),
        });
    }

    let mut total_active = 0_u64;
    let mut total_reclaimed = 0_u64;
    for ((user_id, provider, _), result) in requested_keys.into_iter().zip(results) {
        let [current, reclaimed] = result.as_slice() else {
            return Err(AppError::Redis {
                message: format!(
                    "读取用户实时并发时响应格式无效: user_id={user_id}, provider={provider}"
                ),
            });
        };
        let current = count_from_redis(*current)?;
        let reclaimed = count_from_redis(*reclaimed)?;
        total_active += u64::from(current);
        total_reclaimed += u64::from(reclaimed);
        active_counts
            .counts
            .entry(user_id)
            .or_default()
            .insert(provider.to_owned(), current);
    }

    info!(
        tenant_id = %tenant_id,
        user_count = user_ids.len(),
        provider_count = providers.len(),
        redis_key_count = active_counts.counts.len() * providers.len(),
        total_active,
        reclaimed_expired_leases = total_reclaimed,
        "用户 provider 实时并发已从 Redis 批量读取"
    );

    Ok(active_counts)
}

async fn renew(state: &AppState, identity: &LeaseIdentity) -> AppResult<bool> {
    let mut redis = state.redis();
    let result: Vec<i64> = redis::cmd("EVAL")
        .arg(HEARTBEAT_LUA)
        .arg(1)
        .arg(&identity.redis_key)
        .arg(identity.lease_id.to_string())
        .arg(duration_millis(LEASE_TTL))
        .arg(duration_millis(KEY_TTL))
        .query_async(&mut redis)
        .await
        .map_err(redis_error)?;
    let [renewed, current, reclaimed] = result.as_slice() else {
        return Err(AppError::Redis {
            message: "续租用户并发 lease 时响应格式无效".to_owned(),
        });
    };
    debug!(
        request_id = %identity.request_id,
        lease_id = %identity.lease_id,
        tenant_id = %identity.tenant_id,
        user_id = %identity.user_id,
        provider = identity.provider,
        current = *current,
        reclaimed_expired_leases = *reclaimed,
        renewed = *renewed == 1,
        "用户并发 lease heartbeat 已执行"
    );
    Ok(*renewed == 1)
}

async fn release_identity(state: &AppState, identity: &LeaseIdentity) -> AppResult<()> {
    let mut redis = state.redis();
    let result: Vec<i64> = redis::cmd("EVAL")
        .arg(RELEASE_LUA)
        .arg(1)
        .arg(&identity.redis_key)
        .arg(identity.lease_id.to_string())
        .query_async(&mut redis)
        .await
        .map_err(redis_error)?;
    let [released, current] = result.as_slice() else {
        return Err(AppError::Redis {
            message: "释放用户并发 lease 时响应格式无效".to_owned(),
        });
    };
    info!(
        request_id = %identity.request_id,
        lease_id = %identity.lease_id,
        tenant_id = %identity.tenant_id,
        user_id = %identity.user_id,
        provider = identity.provider,
        release_applied = *released == 1,
        current = *current,
        "用户 provider 并发 lease 已释放"
    );
    Ok(())
}

/// 只接管 proxy 明确返回的流式字节响应。普通响应由 gateway 在返回前释放。
/// 使用现有 Stream 组合器持有 lease，EOF / 错误时先释放再返回终态；不解析 SSE 事件，
/// 不依赖 Content-Length，也不自行处理 HTTP Frame。流尚未开始、读取中或释放中被
/// 丢弃时，组合器持有的 lease 都会通过 Drop 停止续租并后台释放。
pub fn hold_streaming_response(
    response: Response<Body>,
    lease: UserConcurrencyLease,
) -> Response<Body> {
    response.map(|body| {
        let stream = stream::try_unfold(
            (body.into_data_stream(), lease),
            |(mut stream, lease)| async move {
                let terminal = match stream.next().await {
                    Some(Ok(bytes)) => return Ok(Some((bytes, (stream, lease)))),
                    Some(Err(error)) => Err(error),
                    None => Ok(None),
                };
                // 先销毁源流，让上游连接和插件取消收尾立即发生；用户槽位独立释放。
                drop(stream);
                if let Err(error) = lease.release().await {
                    error!(error = %error,
                        "流式响应结束时释放用户并发失败，保留原始终态；RAII guard 已提交兜底释放");
                }
                terminal
            },
        );
        Body::from_stream(stream)
    })
}

fn lease_key(tenant_id: &str, user_id: Uuid, provider: &str) -> String {
    format!("gateway:user-concurrency:{tenant_id}:{user_id}:{provider}:leases")
}

fn duration_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).expect("用户并发 lease duration 必须适合 u64")
}

fn count_from_redis(value: i64) -> AppResult<u32> {
    u32::try_from(value).map_err(|_| AppError::Redis {
        message: format!("Redis 返回无效的用户并发数: {value}"),
    })
}

fn redis_error(source: redis::RedisError) -> AppError {
    AppError::Redis {
        message: source.to_string(),
    }
}
