use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::{
    err::AppResult,
    provider::{maintenance::MaintenanceProvider, resource::UpstreamResourceKind},
    state::AppState,
};

pub(crate) mod store;

/// 管理端使用的通用账号 runtime 视图。
///
/// PostgreSQL 中的 token/quota 时间由 API handler 在生成最终响应时合并；这里仅把 Redis
/// runtime 的公共状态翻译为稳定 DTO，避免各 provider maintenance 重复维护展示逻辑。
#[derive(Debug, Clone, Serialize)]
pub struct AccountRuntimeView {
    pub account_id: Uuid,
    pub runtime_exists: bool,
    pub runtime_ready: bool,
    pub inflight_count: i64,
    pub next_token_refresh_at: Option<DateTime<Utc>>,
    pub quota_resets_at: Option<DateTime<Utc>>,
    pub token_usable: bool,
    pub runtime_state: AccountRuntimeState,
}

impl AccountRuntimeView {
    pub fn missing(account_id: Uuid) -> Self {
        Self {
            account_id,
            runtime_exists: false,
            runtime_ready: false,
            inflight_count: 0,
            next_token_refresh_at: None,
            quota_resets_at: None,
            token_usable: false,
            runtime_state: AccountRuntimeState::Missing,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountRuntimeState {
    Missing,
    Ready,
    TokenRefreshPending,
    QuotaLimited,
    NotRuntime,
}

/// 管理端使用的通用官方 API Key runtime 视图。
///
/// 持久健康状态不另设枚举：`next_probe_at` 为空表示健康，非空表示等待探活。
#[derive(Debug, Clone, Serialize)]
pub struct ApiKeyRuntimeView {
    pub api_key_id: Uuid,
    pub runtime_exists: bool,
    pub runtime_ready: bool,
    pub inflight_count: i64,
    pub next_probe_at: Option<DateTime<Utc>>,
    pub runtime_state: ApiKeyRuntimeState,
}

impl ApiKeyRuntimeView {
    pub fn missing(api_key_id: Uuid) -> Self {
        Self {
            api_key_id,
            runtime_exists: false,
            runtime_ready: false,
            inflight_count: 0,
            next_probe_at: None,
            runtime_state: ApiKeyRuntimeState::Missing,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiKeyRuntimeState {
    Missing,
    Ready,
    PendingProbe,
    NotRuntime,
}

pub async fn account_views<P: MaintenanceProvider>(
    state: &AppState,
    account_ids: &[Uuid],
) -> AppResult<HashMap<Uuid, AccountRuntimeView>> {
    let ids = account_ids
        .iter()
        .copied()
        .map(|id| (UpstreamResourceKind::Account, id))
        .collect::<Vec<_>>();
    let views = store::views(state, P::NAME, &ids).await?;

    Ok(account_ids
        .iter()
        .copied()
        .map(|id| {
            let view = views.get(&(UpstreamResourceKind::Account, id));
            let runtime_exists = view.is_some_and(|view| view.runtime_exists);
            let runtime_ready = view.is_some_and(|view| view.runtime_ready);
            (
                id,
                AccountRuntimeView {
                    account_id: id,
                    runtime_exists,
                    runtime_ready,
                    inflight_count: 0,
                    next_token_refresh_at: None,
                    quota_resets_at: None,
                    token_usable: runtime_ready,
                    runtime_state: if runtime_ready {
                        AccountRuntimeState::Ready
                    } else if runtime_exists {
                        AccountRuntimeState::NotRuntime
                    } else {
                        AccountRuntimeState::Missing
                    },
                },
            )
        })
        .collect())
}

pub async fn api_key_views<P: MaintenanceProvider>(
    state: &AppState,
    api_key_ids: &[Uuid],
) -> AppResult<HashMap<Uuid, ApiKeyRuntimeView>> {
    let ids = api_key_ids
        .iter()
        .copied()
        .map(|id| (UpstreamResourceKind::ApiKey, id))
        .collect::<Vec<_>>();
    let views = store::views(state, P::NAME, &ids).await?;

    Ok(api_key_ids
        .iter()
        .copied()
        .map(|id| {
            let view = views.get(&(UpstreamResourceKind::ApiKey, id));
            let runtime_exists = view.is_some_and(|view| view.runtime_exists);
            let runtime_ready = view.is_some_and(|view| view.runtime_ready);
            (
                id,
                ApiKeyRuntimeView {
                    api_key_id: id,
                    runtime_exists,
                    runtime_ready,
                    inflight_count: 0,
                    next_probe_at: None,
                    runtime_state: if runtime_ready {
                        ApiKeyRuntimeState::Ready
                    } else if runtime_exists {
                        ApiKeyRuntimeState::NotRuntime
                    } else {
                        ApiKeyRuntimeState::Missing
                    },
                },
            )
        })
        .collect())
}
