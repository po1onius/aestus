//! 用户级网关请求并发租约。
//!
//! PostgreSQL 中的最大并发配置是持久真源；本模块只在 Redis 中维护正在执行的请求。
//! 每个租约按 tenant、user、provider 隔离存入 ZSET，member 是单次 acquire 生成的唯一
//! token，score 是 Redis 服务端时间计算出的过期时间。即使用户当前没有配置上限也会登记
//! 租约，因此从“不限”调整为有限值后，已经在途的请求仍会被计入。

use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};

use axum::{
    body::{Body, Bytes, HttpBody},
    http::Response,
};
use hyper::body::{Frame, SizeHint};
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
/// 正常响应通过 [`hold_response`] 托管；若 future 被取消或发生 panic，`Drop` 会提交一次
/// 幂等后台释放，确保租约覆盖完整响应 body 生命周期。
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

    fn release_in_background(mut self, completion: &'static str) {
        self.stop_heartbeat();
        let Some(identity) = self.identity.take() else {
            return;
        };
        let state = self.state.clone();
        debug!(
            request_id = %identity.request_id,
            lease_id = %identity.lease_id,
            tenant_id = %identity.tenant_id,
            user_id = %identity.user_id,
            provider = identity.provider,
            completion,
            "用户并发 lease 已提交后台释放"
        );
        tokio::spawn(async move {
            if let Err(error) = release_identity(&state, &identity).await {
                error!(
                    request_id = %identity.request_id,
                    lease_id = %identity.lease_id,
                    tenant_id = %identity.tenant_id,
                    user_id = %identity.user_id,
                    provider = identity.provider,
                    completion,
                    error = %error,
                    "后台释放用户并发 lease 失败"
                );
            }
        });
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
            "用户并发 lease 在显式释放或响应托管前结束，RAII guard 已提交兜底释放"
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

/// 让并发 lease 覆盖完整的 Axum 响应 body 生命周期。
///
/// wrapper 逐个转发底层 `Frame`，包括 trailers。观察到底层 EOF 或 body error 后，会先
/// 等待 Redis 确认释放，再向 Hyper 返回原始终态，确保正常完成的前一个请求不会与紧随其后
/// 的请求短暂重叠。客户端提前丢弃 body 时仍通过后台幂等释放收尾。
pub fn hold_response(response: Response<Body>, lease: UserConcurrencyLease) -> Response<Body> {
    let identity = lease
        .identity
        .as_ref()
        .expect("交给响应托管的用户并发 lease 必须持有 identity")
        .clone();
    response.map(|inner| {
        Body::new(UserConcurrencyBody {
            inner,
            lease: Some(lease),
            release_future: None,
            terminal: None,
            identity,
        })
    })
}

type ReleaseFuture = Pin<Box<dyn Future<Output = AppResult<()>> + Send + 'static>>;

enum BodyTerminal {
    Eof,
    Error(axum::Error),
}

impl BodyTerminal {
    const fn as_str(&self) -> &'static str {
        match self {
            Self::Eof => "response_body_eof",
            Self::Error(_) => "response_body_error",
        }
    }
}

struct UserConcurrencyBody {
    inner: Body,
    lease: Option<UserConcurrencyLease>,
    release_future: Option<ReleaseFuture>,
    terminal: Option<BodyTerminal>,
    identity: LeaseIdentity,
}

impl HttpBody for UserConcurrencyBody {
    type Data = Bytes;
    type Error = axum::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        loop {
            if let Some(release_future) = self.release_future.as_mut() {
                match release_future.as_mut().poll(cx) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(result) => {
                        self.release_future.take();
                        let terminal = self
                            .terminal
                            .take()
                            .expect("释放 future 完成时必须保存原始响应终态");
                        if let Err(error) = result {
                            error!(
                                request_id = %self.identity.request_id,
                                lease_id = %self.identity.lease_id,
                                tenant_id = %self.identity.tenant_id,
                                user_id = %self.identity.user_id,
                                provider = self.identity.provider,
                                response_terminal = terminal.as_str(),
                                error = %error,
                                "响应终态前等待用户并发 lease 释放失败，原始响应终态继续返回"
                            );
                        }
                        return match terminal {
                            BodyTerminal::Eof => Poll::Ready(None),
                            BodyTerminal::Error(error) => Poll::Ready(Some(Err(error))),
                        };
                    }
                }
            }

            match Pin::new(&mut self.inner).poll_frame(cx) {
                Poll::Ready(None) => {
                    self.terminal = Some(BodyTerminal::Eof);
                }
                Poll::Ready(Some(Err(error))) => {
                    self.terminal = Some(BodyTerminal::Error(error));
                }
                other => return other,
            }

            let lease = self
                .lease
                .take()
                .expect("首次观察到响应终态时必须仍持有用户并发 lease");
            self.release_future = Some(Box::pin(lease.release()));
        }
    }

    fn is_end_stream(&self) -> bool {
        // 即使底层已经知道自己为空，也必须让 Hyper 至少 poll 一次本 wrapper，等待 Redis
        // 释放确认后再观察到真正 EOF。
        self.lease.is_none()
            && self.release_future.is_none()
            && self.terminal.is_none()
            && self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

impl Drop for UserConcurrencyBody {
    fn drop(&mut self) {
        if let Some(lease) = self.lease.take() {
            lease.release_in_background("response_body_dropped");
        } else if self.release_future.is_some() {
            debug!(
                request_id = %self.identity.request_id,
                lease_id = %self.identity.lease_id,
                tenant_id = %self.identity.tenant_id,
                user_id = %self.identity.user_id,
                provider = self.identity.provider,
                "响应 body 在等待用户并发 lease 显式释放期间被丢弃，将由 RAII guard 兜底释放"
            );
        }
    }
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
