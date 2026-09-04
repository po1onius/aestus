use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::{
    err::{AppError, AppResult},
    provider::credential::normalize_provider,
    state::AppState,
};

const OAUTH_SESSION_KEY_PREFIX: &str = "aestus:provider:oauth_session";

/// provider 共用的短期 OAuth PKCE 会话。
///
/// OAuth 握手状态不属于持久业务数据，因此只保存在 Redis，并由 key TTL 自动回收。
/// 分组只在最终创建账号时参与 PostgreSQL 校验，不进入 OAuth 会话，避免临时授权流程
/// 反向影响分组归档等持久资源生命周期。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderOauthSession {
    pub provider: String,
    pub tenant_id: String,
    pub pkce_verifier: String,
    pub redirect_uri: String,
    pub expires_at: DateTime<Utc>,
}

/// 创建一次性 OAuth 会话。
///
/// `NX` 防止极低概率的随机 state 碰撞覆盖现有 PKCE verifier，`EX` 让 Redis 负责准确
/// 回收未完成流程。授权码、callback URL 和最终 token 都不会写入该 key。
pub async fn create(
    app_state: &AppState,
    provider: &str,
    tenant_id: String,
    oauth_state: &str,
    pkce_verifier: String,
    redirect_uri: String,
    expires_at: DateTime<Utc>,
) -> AppResult<()> {
    let provider = normalize_provider(provider.to_owned())?;
    let ttl_seconds = (expires_at - Utc::now()).num_seconds();
    if ttl_seconds <= 0 {
        warn!(
            provider,
            oauth_state,
            expires_at = %expires_at,
            "OAuth 临时会话 TTL 无效，拒绝写入 Redis"
        );
        return Err(AppError::Redis {
            message: "OAuth 临时会话过期时间必须晚于当前时间，请检查对应 AESTUS_*_OAUTH_SESSION_TTL_SECONDS 配置".to_owned(),
        });
    }

    let session = ProviderOauthSession {
        provider: provider.clone(),
        tenant_id: tenant_id.clone(),
        pkce_verifier,
        redirect_uri,
        expires_at,
    };
    let payload = serde_json::to_string(&session).map_err(|source| AppError::Redis {
        message: format!("序列化 OAuth 临时会话失败: {source}"),
    })?;
    let key = session_key(&provider, oauth_state);
    let mut redis = app_state.redis();
    let created: Option<String> = redis::cmd("SET")
        .arg(&key)
        .arg(payload)
        .arg("NX")
        .arg("EX")
        .arg(ttl_seconds)
        .query_async(&mut redis)
        .await
        .map_err(redis_error)?;
    if created.is_none() {
        warn!(
            provider,
            oauth_state, "OAuth state 已存在，拒绝覆盖 Redis 临时会话"
        );
        return Err(AppError::Redis {
            message: "OAuth state 随机值发生冲突，请重新生成授权链接".to_owned(),
        });
    }

    info!(
        provider,
        tenant_id = %tenant_id,
        oauth_state,
        expires_at = %session.expires_at,
        ttl_seconds,
        "provider OAuth 临时会话已写入 Redis"
    );
    Ok(())
}

/// 原子取得并删除 OAuth 会话，确保同一个 state 最多只能交换一次授权码。
///
/// 当前部署基线为 Redis 8，直接使用 Redis 6.2 起提供的 `GETDEL`。会话在任何后续
/// 上游或数据库步骤失败时也不会恢复，这与 authorization code 的一次性语义一致。
pub async fn take(
    app_state: &AppState,
    provider: &str,
    oauth_state: &str,
) -> AppResult<Option<ProviderOauthSession>> {
    let provider = normalize_provider(provider.to_owned())?;
    let key = session_key(&provider, oauth_state);
    let mut redis = app_state.redis();
    let payload: Option<String> = redis::cmd("GETDEL")
        .arg(&key)
        .query_async(&mut redis)
        .await
        .map_err(redis_error)?;
    let Some(payload) = payload else {
        warn!(
            provider,
            oauth_state, "provider OAuth Redis 临时会话不存在、已过期或已消费"
        );
        return Ok(None);
    };

    let session = serde_json::from_str::<ProviderOauthSession>(&payload).map_err(|source| {
        warn!(
            provider,
            oauth_state,
            error = %source,
            "provider OAuth Redis 临时会话格式无效，已按一次性语义删除"
        );
        AppError::Redis {
            message: format!("解析 OAuth 临时会话失败: {source}"),
        }
    })?;
    if session.provider != provider {
        warn!(
            expected_provider = provider,
            actual_provider = session.provider,
            oauth_state,
            "provider OAuth Redis 临时会话的 provider 不一致，已拒绝使用"
        );
        return Err(AppError::Redis {
            message: "OAuth 临时会话 provider 与 Redis key 不一致".to_owned(),
        });
    }
    if session.expires_at <= Utc::now() {
        warn!(
            provider,
            oauth_state,
            expires_at = %session.expires_at,
            "provider OAuth Redis 临时会话已过期"
        );
        return Ok(None);
    }

    info!(
        provider,
        oauth_state,
        expires_at = %session.expires_at,
        "provider OAuth Redis 临时会话已原子消费"
    );
    Ok(Some(session))
}

fn session_key(provider: &str, oauth_state: &str) -> String {
    format!("{OAUTH_SESSION_KEY_PREFIX}:{provider}:{oauth_state}")
}

fn redis_error(source: redis::RedisError) -> AppError {
    AppError::Redis {
        message: source.to_string(),
    }
}
