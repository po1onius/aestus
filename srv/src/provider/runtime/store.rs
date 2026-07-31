use std::collections::HashMap;

use rand::RngExt;
use serde::Serialize;
use tracing::{info, warn};
use uuid::Uuid;

use crate::{
    err::{AppError, AppResult},
    infra::redis::RedisConnection,
    provider::resource::{UpstreamResource, UpstreamResourceKind},
    state::AppState,
};

const RUNTIME_SAMPLE_ATTEMPTS: usize = 2;

const RELEASE_LOCK_LUA: &str = r#"
if redis.call('GET', KEYS[1]) == ARGV[1] then
    return redis.call('DEL', KEYS[1])
end
return 0
"#;

/// runtime payload、ready index 与投影版本必须在一个 Redis 原子操作内发布。相同输入
/// 重放保持幂等；较旧投影和同版本 tombstone 都不能复活已经摘除的资源。
const PUBLISH_RUNTIME_LUA: &str = r#"
local current = redis.call('HGET', KEYS[3], ARGV[1])
if current then
    local current_revision = tonumber(current)
    local incoming_revision = tonumber(ARGV[3])
    if current_revision > incoming_revision then
        return -1
    end
    if current_revision == incoming_revision and redis.call('EXISTS', KEYS[1]) == 0 then
        return -2
    end
end
local previous_index_member = redis.call('HGET', KEYS[1], 'index_member')
if previous_index_member then
    redis.call('ZREM', KEYS[2], previous_index_member)
end
redis.call('HSET', KEYS[1], 'payload', ARGV[4], 'revision', ARGV[3], 'index_member', ARGV[2])
redis.call('HSET', KEYS[3], ARGV[1], ARGV[3])
redis.call('ZADD', KEYS[2], 0, ARGV[2])
return 1
"#;

/// 摘除不晚于传入投影版本的 runtime，并把该版本保留为 tombstone。重复摘除得到相同
/// 最终状态，新投影则由版本栅栏保护。
const REMOVE_RUNTIME_AT_OR_BEFORE_REVISION_LUA: &str = r#"
local current = redis.call('HGET', KEYS[3], ARGV[1])
if current and tonumber(current) > tonumber(ARGV[2]) then
    return -1
end
local previous_index_member = redis.call('HGET', KEYS[1], 'index_member')
if previous_index_member then
    redis.call('ZREM', KEYS[2], previous_index_member)
end
redis.call('DEL', KEYS[1])
redis.call('HSET', KEYS[3], ARGV[1], ARGV[2])
return 1
"#;

const REMOVE_RESOURCE_RUNTIME_LUA: &str = r#"
local previous_index_member = redis.call('HGET', KEYS[1], 'index_member')
if previous_index_member then
    redis.call('ZREM', KEYS[2], previous_index_member)
end
redis.call('DEL', KEYS[1])
local current = redis.call('HGET', KEYS[3], ARGV[1])
if not current or tonumber(current) < tonumber(ARGV[2]) then
    redis.call('HSET', KEYS[3], ARGV[1], ARGV[2])
end
return 1
"#;

// Redis Lua 5.1 以 IEEE-754 double 表示 number。该值是可精确表达的最大整数，作为
// 删除 tombstone 可永久拒绝所有基于 PostgreSQL 微秒时间戳生成的正常投影版本。
const DELETED_RUNTIME_REVISION: i64 = 9_007_199_254_740_991;

/// Redis runtime 的中立管理视图。账号刷新时间、quota reset 和 API Key 探活时间均为
/// PostgreSQL 持久事实，由 service/HTTP DTO 合并，不进入调度热路径。
#[derive(Debug, Clone, Serialize)]
pub(crate) struct RuntimeView {
    pub resource_id: Uuid,
    pub resource_type: UpstreamResourceKind,
    pub runtime_exists: bool,
    pub runtime_ready: bool,
    pub revision: Option<i64>,
}

impl RuntimeView {
    fn missing(resource_id: Uuid, resource_type: UpstreamResourceKind) -> Self {
        Self {
            resource_id,
            resource_type,
            runtime_exists: false,
            runtime_ready: false,
            revision: None,
        }
    }
}

/// 带随机所有权值的 Redis 锁 token。释放时必须同时匹配 key 和 value，避免旧任务在
/// TTL 过期后误删新任务已经取得的锁。
pub(crate) struct LockToken {
    key: String,
    value: String,
}

pub(crate) async fn try_resource_lock(
    state: &AppState,
    provider: &str,
    kind: UpstreamResourceKind,
    id: Uuid,
    ttl_seconds: u64,
) -> AppResult<Option<LockToken>> {
    try_lock(state, resource_lock_key(provider, kind, id), ttl_seconds).await
}

pub(crate) async fn release_lock(state: &AppState, lock: LockToken) {
    let mut redis = state.redis();
    let result: Result<i64, redis::RedisError> = redis::cmd("EVAL")
        .arg(RELEASE_LOCK_LUA)
        .arg(1)
        .arg(&lock.key)
        .arg(&lock.value)
        .query_async(&mut redis)
        .await;
    if let Err(error) = result {
        warn!(lock_key = %lock.key, error = %error, "释放 provider Redis 锁失败，将等待 TTL 自动过期");
    }
}

async fn try_lock(state: &AppState, key: String, ttl_seconds: u64) -> AppResult<Option<LockToken>> {
    let value = Uuid::now_v7().to_string();
    let mut redis = state.redis();
    let acquired: Option<String> = redis::cmd("SET")
        .arg(&key)
        .arg(&value)
        .arg("NX")
        .arg("EX")
        .arg(ttl_seconds.max(1))
        .query_async(&mut redis)
        .await
        .map_err(redis_error)?;
    Ok(acquired.map(|_| LockToken { key, value }))
}

pub(crate) async fn publish(state: &AppState, runtime: UpstreamResource) -> AppResult<bool> {
    if runtime.auth_secret.is_empty() {
        remove(state, &runtime).await?;
        return Ok(false);
    }
    let payload = serialize_runtime(&runtime)?;
    let member = runtime.resource_member();
    let index_member = runtime_index_member(runtime.group_id, &member);
    let mut redis = state.redis();
    let result: i64 = redis::cmd("EVAL")
        .arg(PUBLISH_RUNTIME_LUA)
        .arg(3)
        .arg(runtime_key(&runtime.provider, &member))
        .arg(runtime_index_key(&runtime.provider))
        .arg(runtime_revision_key(&runtime.provider))
        .arg(&member)
        .arg(&index_member)
        .arg(runtime.revision)
        .arg(payload)
        .query_async(&mut redis)
        .await
        .map_err(redis_error)?;
    if result == 1 {
        info!(
            provider = %runtime.provider,
            resource_type = runtime.kind.as_str(),
            resource_id = %runtime.id,
            provider_group_id = %runtime.group_id,
            projection_version = runtime.revision,
            credential_generation = ?runtime.credential_generation,
            "上游资源 runtime 与 ready index 已原子发布"
        );
        return Ok(true);
    }

    info!(
        provider = %runtime.provider,
        resource_type = runtime.kind.as_str(),
        resource_id = %runtime.id,
        projection_version = runtime.revision,
        rejection = if result == -2 { "same_version_tombstone" } else { "newer_projection" },
        "上游资源 runtime 发布被投影版本栅栏拒绝"
    );
    Ok(false)
}

pub(crate) async fn remove_at_or_before_revision(
    state: &AppState,
    provider: &str,
    kind: UpstreamResourceKind,
    id: Uuid,
    revision: i64,
) -> AppResult<bool> {
    let member = resource_member(kind, id);
    let mut redis = state.redis();
    let result: i64 = redis::cmd("EVAL")
        .arg(REMOVE_RUNTIME_AT_OR_BEFORE_REVISION_LUA)
        .arg(3)
        .arg(runtime_key(provider, &member))
        .arg(runtime_index_key(provider))
        .arg(runtime_revision_key(provider))
        .arg(&member)
        .arg(revision)
        .query_async(&mut redis)
        .await
        .map_err(redis_error)?;
    if result == -1 {
        info!(
            provider,
            resource_type = kind.as_str(),
            resource_id = %id,
            projection_version = revision,
            "上游资源摘除被更新的投影版本栅栏拒绝"
        );
        return Ok(false);
    }
    Ok(true)
}

pub(crate) async fn remove(state: &AppState, runtime: &UpstreamResource) -> AppResult<bool> {
    remove_at_or_before_revision(
        state,
        &runtime.provider,
        runtime.kind,
        runtime.id,
        runtime.revision,
    )
    .await
}

pub(crate) async fn delete_resource(
    state: &AppState,
    provider: &str,
    kind: UpstreamResourceKind,
    id: Uuid,
) -> AppResult<()> {
    let member = resource_member(kind, id);
    let mut redis = state.redis();
    let _: i64 = redis::cmd("EVAL")
        .arg(REMOVE_RESOURCE_RUNTIME_LUA)
        .arg(3)
        .arg(runtime_key(provider, &member))
        .arg(runtime_index_key(provider))
        .arg(runtime_revision_key(provider))
        .arg(&member)
        .arg(DELETED_RUNTIME_REVISION)
        .query_async(&mut redis)
        .await
        .map_err(redis_error)?;
    info!(
        provider,
        resource_type = kind.as_str(),
        resource_id = %id,
        "上游资源 runtime 已删除并写入永久投影 tombstone"
    );
    Ok(())
}

pub(crate) async fn load_resources(
    state: &AppState,
    redis: &mut RedisConnection,
    provider: &str,
    group_id: Uuid,
    minimum_candidate_limit: usize,
) -> AppResult<Vec<UpstreamResource>> {
    let candidate_limit = (state.config().provider_scheduler_candidate_limit.max(1) as usize)
        .max(minimum_candidate_limit.max(1));
    let index_key = runtime_index_key(provider);
    let (lex_min, lex_max) = runtime_group_lex_range(group_id);

    for attempt in 0..RUNTIME_SAMPLE_ATTEMPTS {
        let member_count: usize = redis::cmd("ZLEXCOUNT")
            .arg(&index_key)
            .arg(&lex_min)
            .arg(&lex_max)
            .query_async(&mut *redis)
            .await
            .map_err(redis_error)?;
        if member_count == 0 {
            return Ok(Vec::new());
        }
        let max_offset = member_count.saturating_sub(candidate_limit);
        let offset = if max_offset == 0 {
            0
        } else {
            rand::rng().random_range(0..=max_offset)
        };
        let indexed_members: Vec<String> = redis::cmd("ZRANGEBYLEX")
            .arg(&index_key)
            .arg(&lex_min)
            .arg(&lex_max)
            .arg("LIMIT")
            .arg(offset)
            .arg(candidate_limit)
            .query_async(&mut *redis)
            .await
            .map_err(redis_error)?;
        let members = indexed_members
            .iter()
            .filter_map(|indexed_member| {
                indexed_member
                    .split_once('|')
                    .map(|(_, member)| member.to_owned())
            })
            .collect::<Vec<_>>();
        if members.len() != indexed_members.len() {
            return Err(AppError::Redis {
                message: "Provider 分组 ready index 包含非法成员".to_owned(),
            });
        }

        // runtime 分散保存在每个资源自己的 Redis hash 中，无法直接使用 HMGET。这里把所有
        // HGET 打包为一次 pipeline 往返，保持逐资源 payload 的存储边界，同时消除候选数
        // 线性增长的网络等待。
        let mut pipe = redis::pipe();
        for member in &members {
            pipe.cmd("HGET")
                .arg(runtime_key(provider, member))
                .arg("payload");
        }
        let payloads: Vec<Option<String>> =
            pipe.query_async(&mut *redis).await.map_err(redis_error)?;
        if payloads.len() != members.len() {
            return Err(AppError::Redis {
                message: "批量读取 scheduler 候选 runtime 时响应数量不匹配".to_owned(),
            });
        }

        let mut resources = Vec::with_capacity(members.len());
        let mut stale_count = 0usize;
        for (member, payload) in members.into_iter().zip(payloads) {
            match parse_runtime_payload(provider, &member, payload)? {
                Some(runtime) if runtime.group_id == group_id => resources.push(runtime),
                Some(runtime) => {
                    stale_count += 1;
                    warn!(
                        provider,
                        provider_group_id = %group_id,
                        payload_provider_group_id = %runtime.group_id,
                        runtime_member = %member,
                        "分组 ready index 与 runtime payload 不一致，候选已忽略"
                    );
                }
                None => {
                    stale_count += 1;
                    // 读取 payload 与修改 ready index 不是同一个原子操作；此处不能清理，
                    // 否则可能误删两次命令之间刚发布的新投影。
                    warn!(
                        provider,
                        runtime_member = %member,
                        "runtime 抽样命中缺少有效 payload 的 ready 成员，已忽略且未修改 index"
                    );
                }
            }
        }
        if !resources.is_empty() || stale_count == 0 {
            return Ok(resources);
        }
        info!(
            provider,
            sample_attempt = attempt + 1,
            stale_count,
            "runtime 抽样仅命中脏成员，重新抽样"
        );
    }
    Ok(Vec::new())
}

pub(crate) async fn read_indexed(
    redis: &mut RedisConnection,
    provider: &str,
    group_id: Uuid,
    member: &str,
) -> AppResult<Option<UpstreamResource>> {
    // sticky 命中仍需同时确认 ready index 与 payload。两个读取没有跨命令写依赖，使用
    // pipeline 合并网络往返；解析和脏成员处理语义保持不变。
    let (ready_score, payload): (Option<f64>, Option<String>) = redis::pipe()
        .cmd("ZSCORE")
        .arg(runtime_index_key(provider))
        .arg(runtime_index_member(group_id, member))
        .cmd("HGET")
        .arg(runtime_key(provider, member))
        .arg("payload")
        .query_async(&mut *redis)
        .await
        .map_err(redis_error)?;
    if ready_score.is_none() {
        return Ok(None);
    }

    let runtime = parse_runtime_payload(provider, member, payload)?
        .filter(|runtime| runtime.group_id == group_id);
    if runtime.is_none() {
        warn!(
            provider,
            runtime_member = member,
            "sticky ready 成员缺少有效 runtime payload，已按未命中处理且未修改 index"
        );
    }
    Ok(runtime)
}

pub(crate) async fn views(
    state: &AppState,
    provider: &str,
    ids: &[(UpstreamResourceKind, Uuid)],
) -> AppResult<HashMap<(UpstreamResourceKind, Uuid), RuntimeView>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }

    let mut redis = state.redis();
    let members = ids
        .iter()
        .map(|(kind, id)| resource_member(*kind, *id))
        .collect::<Vec<_>>();
    let index_key = runtime_index_key(provider);
    let mut runtime_pipe = redis::pipe();
    for member in &members {
        runtime_pipe
            .cmd("HGET")
            .arg(runtime_key(provider, member))
            .arg("payload");
    }
    let payloads: Vec<Option<String>> = runtime_pipe
        .query_async(&mut redis)
        .await
        .map_err(redis_error)?;
    if payloads.len() != ids.len() {
        return Err(AppError::Redis {
            message: "批量读取 provider runtime payload 数量不匹配".to_owned(),
        });
    }
    let runtimes = payloads
        .into_iter()
        .enumerate()
        .map(|(index, payload)| parse_runtime_payload(provider, &members[index], payload))
        .collect::<AppResult<Vec<_>>>()?;
    let mut ready_pipe = redis::pipe();
    for (index, runtime) in runtimes.iter().enumerate() {
        let index_member = runtime.as_ref().map_or_else(
            || runtime_index_member(Uuid::nil(), &members[index]),
            |runtime| runtime_index_member(runtime.group_id, &members[index]),
        );
        ready_pipe.cmd("ZSCORE").arg(&index_key).arg(index_member);
    }
    let ready_scores: Vec<Option<f64>> = ready_pipe
        .query_async(&mut redis)
        .await
        .map_err(redis_error)?;
    if ready_scores.len() != ids.len() {
        return Err(AppError::Redis {
            message: "批量读取 provider runtime 视图时响应数量不匹配".to_owned(),
        });
    }

    let mut views = HashMap::with_capacity(ids.len());
    let mut invalid_ready_count = 0usize;
    for (index, runtime) in runtimes.into_iter().enumerate() {
        let (kind, id) = ids[index];
        let ready = ready_scores[index].is_some();
        let view = match runtime {
            Some(runtime) => RuntimeView {
                resource_id: id,
                resource_type: kind,
                runtime_exists: true,
                runtime_ready: ready,
                revision: Some(runtime.revision),
            },
            None => {
                if ready {
                    invalid_ready_count += 1;
                }
                RuntimeView::missing(id, kind)
            }
        };
        views.insert((kind, id), view);
    }
    if invalid_ready_count > 0 {
        warn!(
            provider,
            invalid_ready_count,
            "批量 runtime 视图发现 ready 成员缺少有效 payload，已展示为未就绪且未修改 index"
        );
    }
    Ok(views)
}

pub(crate) async fn clear_runtime_index(state: &AppState, provider: &str) -> AppResult<()> {
    let mut redis = state.redis();
    let index_key = runtime_index_key(provider);
    let indexed_members: Vec<String> = redis::cmd("ZRANGE")
        .arg(&index_key)
        .arg(0)
        .arg(-1)
        .query_async(&mut redis)
        .await
        .map_err(redis_error)?;
    let mut pipe = redis::pipe();
    for indexed_member in indexed_members {
        if let Some((_, member)) = indexed_member.split_once('|') {
            pipe.cmd("DEL").arg(runtime_key(provider, member)).ignore();
        }
    }
    pipe.cmd("DEL").arg(index_key).ignore();
    pipe.cmd("DEL").arg(runtime_revision_key(provider)).ignore();
    let _: () = pipe.query_async(&mut redis).await.map_err(redis_error)?;
    Ok(())
}

fn resource_member(kind: UpstreamResourceKind, id: Uuid) -> String {
    format!("{}:{id}", kind.as_str())
}

fn parse_runtime_payload(
    provider: &str,
    member: &str,
    payload: Option<String>,
) -> AppResult<Option<UpstreamResource>> {
    let Some(payload) = payload else {
        return Ok(None);
    };
    let runtime =
        serde_json::from_str::<UpstreamResource>(&payload).map_err(|source| AppError::Redis {
            message: format!(
                "解析上游资源 runtime 失败: provider={provider}, member={member}, {source}"
            ),
        })?;
    if runtime.provider != provider
        || runtime.resource_member() != member
        || runtime.auth_secret.trim().is_empty()
        || !runtime.request_context.is_object()
    {
        warn!(
            provider,
            runtime_member = member,
            payload_provider = %runtime.provider,
            payload_resource_type = runtime.kind.as_str(),
            payload_resource_id = %runtime.id,
            "上游资源 runtime payload 与 Redis key 不一致或核心字段无效，读路径已忽略"
        );
        return Ok(None);
    }
    Ok(Some(runtime))
}

fn serialize_runtime(runtime: &UpstreamResource) -> AppResult<String> {
    serde_json::to_string(runtime).map_err(|source| AppError::Redis {
        message: format!("序列化上游资源 runtime 失败: {source}"),
    })
}

fn runtime_index_key(provider: &str) -> String {
    format!("provider:{provider}:resources:runtime_by_group")
}

fn runtime_index_member(group_id: Uuid, member: &str) -> String {
    format!("{group_id}|{member}")
}

fn runtime_group_lex_range(group_id: Uuid) -> (String, String) {
    let prefix = format!("{group_id}|");
    (format!("[{prefix}"), format!("[{prefix}\u{10ffff}"))
}

fn runtime_revision_key(provider: &str) -> String {
    format!("provider:{provider}:resources:runtime_revision")
}

fn runtime_key(provider: &str, member: &str) -> String {
    format!("provider:{provider}:resource:runtime:{member}")
}

fn resource_lock_key(provider: &str, kind: UpstreamResourceKind, id: Uuid) -> String {
    format!(
        "provider:{provider}:resource:maintenance-lock:{}:{id}",
        kind.as_str()
    )
}

fn redis_error(source: redis::RedisError) -> AppError {
    AppError::Redis {
        message: source.to_string(),
    }
}
