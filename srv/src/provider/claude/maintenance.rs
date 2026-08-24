use chrono::{DateTime, Utc};
use tracing::{info, warn};
use uuid::Uuid;

use crate::{
    err::{AppError, AppResult},
    provider::{
        claude::{
            auth::{self, TokenFailureKind},
            messages_http::{build_upstream_url, request_header as claude_header},
            model::{ClaudeAccountRequestContext, ClaudeAccountSpecific, PROVIDER},
        },
        credential::{ProviderAccount, ProviderApiKey, serialize_specific},
        maintenance::{
            MaintenanceFailure, MaintenanceFailureKind, MaintenanceProvider, RefreshedAccount,
            ResourceFeedback,
        },
        protocol::UpstreamFeedback,
        resource::{UpstreamResource, UpstreamResourceKind, serialize_request_context},
        response_logging::response_body_for_tracing,
    },
    state::AppState,
};

pub struct ClaudeMaintenance;

const API_KEY_PROBE_PROMPT: &str = "Hello, Claude";
const API_KEY_PROBE_MAX_TOKENS: u64 = 1024;

impl MaintenanceProvider for ClaudeMaintenance {
    const NAME: &'static str = PROVIDER;

    fn account_request_context(account: &ProviderAccount) -> AppResult<serde_json::Value> {
        let specific = account.parse_specific::<ClaudeAccountSpecific>()?;
        let raw_account_uuid =
            specific
                .account_uuid
                .as_deref()
                .ok_or_else(|| AppError::BadRequest {
                    message: format!(
                        "Claude 账号缺少 Profile account UUID，无法发布请求 runtime: account_id={}",
                        account.id
                    ),
                })?;
        let account_uuid = Uuid::parse_str(raw_account_uuid).map_err(|source| {
            AppError::BadRequest {
                message: format!(
                    "Claude 账号 Profile account UUID 无效，无法发布请求 runtime: account_id={}, error={source}",
                    account.id
                ),
            }
        })?;
        if account_uuid.is_nil() {
            return Err(AppError::BadRequest {
                message: format!(
                    "Claude 账号 Profile account UUID 不能是 nil UUID: account_id={}",
                    account.id
                ),
            });
        }

        // Redis 仅投射上游请求必须使用的 account UUID；订阅、邮箱、计费和时间等字段
        // 继续只保存在 PostgreSQL specific 中。
        serialize_request_context(&ClaudeAccountRequestContext { account_uuid })
    }

    async fn refresh_account<'a>(
        state: &'a AppState,
        account: &'a ProviderAccount,
    ) -> Result<RefreshedAccount, MaintenanceFailure> {
        let current = account
            .parse_specific::<ClaudeAccountSpecific>()
            .map_err(bad_response)?;
        let grant = auth::refresh_token(state, &account.refresh_token, &account.client_id)
            .await
            .map_err(|error| MaintenanceFailure {
                kind: match error.kind() {
                    TokenFailureKind::InvalidGrant => MaintenanceFailureKind::Unauthorized,
                    TokenFailureKind::RateLimited => MaintenanceFailureKind::RateLimited,
                    TokenFailureKind::Retryable => MaintenanceFailureKind::Retryable,
                    TokenFailureKind::BadResponse => MaintenanceFailureKind::BadResponse,
                },
                message: error.to_string(),
            })?;
        let next_token_refresh_at = grant.access_token_expires_at
            - chrono::Duration::seconds(state.config().claude_token_refresh_ahead_seconds as i64);
        if next_token_refresh_at <= Utc::now() {
            return Err(MaintenanceFailure {
                kind: MaintenanceFailureKind::BadResponse,
                message: format!(
                    "Claude refresh 响应 access token 已进入刷新窗口: {next_token_refresh_at}"
                ),
            });
        }
        let specific = ClaudeAccountSpecific {
            // Profile 已验证的身份是账号去重依据，refresh 响应不得静默改写它。旧数据若
            // 尚无身份字段，才使用 refresh grant 返回值补齐。
            account_uuid: current.account_uuid.or(grant.account_uuid),
            organization_uuid: current.organization_uuid.or(grant.organization_uuid),
            email_address: grant.email_address.or(current.email_address),
            scopes: grant.scopes.unwrap_or(current.scopes),
            refresh_token_expires_at: grant
                .refresh_token_expires_at
                .or(current.refresh_token_expires_at),
            display_name: current.display_name,
            subscription_type: current.subscription_type,
            rate_limit_tier: current.rate_limit_tier,
            has_extra_usage_enabled: current.has_extra_usage_enabled,
            billing_type: current.billing_type,
            account_created_at: current.account_created_at,
            subscription_created_at: current.subscription_created_at,
        };
        Ok(RefreshedAccount {
            refresh_token: grant
                .refresh_token
                .unwrap_or_else(|| account.refresh_token.clone()),
            access_token: grant.access_token,
            next_token_refresh_at,
            specific: serialize_specific(&specific).map_err(bad_response)?,
        })
    }

    async fn probe_api_key<'a>(
        state: &'a AppState,
        api_key: &'a ProviderApiKey,
    ) -> Result<(), MaintenanceFailure> {
        let url = build_upstream_url(
            &api_key.base_url,
            &state.config().claude_upstream_messages_path,
            None,
        );
        // 探活请求由网关自身生成，不属于调用方透明代理；继续使用完整的 Anthropic
        // Messages 协议基础 header，确保探活只检验 API Key/资源是否可用。
        let mut headers = claude_header::build_api_key_probe_headers();
        claude_header::inject_official_api_key_credential(&mut headers, &api_key.api_key)
            .map_err(bad_response)?;
        info!(
            provider_api_key_id = %api_key.id,
            upstream_url = %url,
            probe_model = %state.config().claude_upstream_api_key_probe_model,
            "Claude 官方 API Key 开始通过单次正式 Messages 请求探活"
        );
        // 请求头与请求体严格采用 Anthropic 官方 Messages curl 示例的形状。明确只发送
        // 一次 HTTP 请求；不使用 Anthropic SDK，也不在 provider 私有实现中添加重试。
        // 后续重新探活完全由通用 maintenance 的 next_probe_at 驱动。
        let response = state
            .http_client()
            .post(&url)
            .headers(headers)
            .json(&serde_json::json!({
                "model": state.config().claude_upstream_api_key_probe_model,
                "max_tokens": API_KEY_PROBE_MAX_TOKENS,
                "messages": [{"role": "user", "content": API_KEY_PROBE_PROMPT}],
            }))
            .send()
            .await
            .map_err(|source| MaintenanceFailure {
                kind: MaintenanceFailureKind::Retryable,
                message: format!("Claude API Key Messages 探活请求失败: {source}"),
            })?;
        let status = response.status();
        let body = response
            .bytes()
            .await
            .map_err(|source| MaintenanceFailure {
                kind: MaintenanceFailureKind::Retryable,
                message: format!("读取 Claude API Key 探活响应体失败: {source}"),
            })?;
        if status.is_success() {
            info!(
                provider_api_key_id = %api_key.id,
                upstream_status = status.as_u16(),
                response_bytes = body.len(),
                probe_model = %state.config().claude_upstream_api_key_probe_model,
                "Claude 官方 API Key 正式 Messages 探活成功"
            );
            return Ok(());
        }

        let upstream_error = safe_probe_error_type(&body);
        let tracing_body = response_body_for_tracing(&body);
        warn!(
            provider_api_key_id = %api_key.id,
            upstream_status = status.as_u16(),
            response_bytes = body.len(),
            upstream_error,
            upstream_response_body_encoding = tracing_body.encoding(),
            upstream_response_body = %tracing_body.content(),
            "Claude 官方 API Key 探活失败，完整响应正文已写入 tracing"
        );
        Err(MaintenanceFailure {
            kind: if matches!(
                status,
                reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN
            ) {
                MaintenanceFailureKind::Unauthorized
            } else if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                MaintenanceFailureKind::RateLimited
            } else if status == reqwest::StatusCode::REQUEST_TIMEOUT
                || status == reqwest::StatusCode::CONFLICT
                || status.is_server_error()
            {
                MaintenanceFailureKind::Retryable
            } else {
                MaintenanceFailureKind::BadResponse
            },
            message: format!(
                "Claude API Key Messages 探活返回失败状态: status={status}, error={upstream_error}"
            ),
        })
    }

    fn account_refresh_retry_seconds(state: &AppState) -> u64 {
        state.config().claude_token_refresh_retry_seconds
    }

    fn api_key_probe_interval_seconds(state: &AppState) -> u64 {
        state
            .config()
            .claude_upstream_api_key_probe_interval_seconds
    }

    fn feedback_command(
        state: &AppState,
        request_id: Uuid,
        resource: &UpstreamResource,
        feedback: UpstreamFeedback,
    ) -> AppResult<Option<ResourceFeedback>> {
        if resource.kind == UpstreamResourceKind::ApiKey {
            // Claude 官方 Key 的维护策略保留在 provider 内部：明确的凭证、权限、计费和
            // 配额错误进入 Error/认证/限流类回执并暂停调度；网络失败已经在通用传输层
            // 收口且不会到达 provider，公共上游临时故障也不能触发 Key 隔离或探活。
            return Ok(match feedback {
                UpstreamFeedback::Error { reason }
                | UpstreamFeedback::AuthenticationRejected { reason }
                | UpstreamFeedback::RateLimited { reason, .. }
                | UpstreamFeedback::QuotaExhausted { reason, .. }
                | UpstreamFeedback::EntitlementMissing { reason } => {
                    Some(ResourceFeedback::ApiKeyError { reason })
                }
                UpstreamFeedback::TemporarilyUnavailable { .. } => None,
            });
        }
        let command = match feedback {
            UpstreamFeedback::Error { .. } => None,
            UpstreamFeedback::AuthenticationRejected { reason } => {
                Some(ResourceFeedback::AccountUnauthorized {
                    reason: format!(
                        "Claude 请求期上游拒绝认证: request_id={request_id}, reason={reason}"
                    ),
                })
            }
            UpstreamFeedback::RateLimited { resets_at, .. }
            | UpstreamFeedback::QuotaExhausted { resets_at, .. } => {
                Some(ResourceFeedback::AccountQuotaLimited {
                    resets_at: resets_at.unwrap_or_else(|| {
                        Utc::now()
                            + chrono::Duration::seconds(
                                state
                                    .config()
                                    .claude_account_rate_limit_cooldown_seconds
                                    .max(1) as i64,
                            )
                    }),
                })
            }
            UpstreamFeedback::EntitlementMissing { reason } => {
                // billing_error 可能在订阅或额外用量恢复后消失；permission_error 也可能只
                // 针对当前模型、beta 或 Workspace。这里不能把账号永久标成 invalid，也不
                // 能误触发 token refresh。先复用账号现有的可恢复调度门闩做短期隔离，
                // ticker 到期后自动重新发布；本次请求会立即改选其他资源。
                let cooldown_seconds = state
                    .config()
                    .claude_account_rate_limit_cooldown_seconds
                    .max(1);
                let resets_at = Utc::now() + chrono::Duration::seconds(cooldown_seconds as i64);
                warn!(
                    request_id = %request_id,
                    provider_account_id = %resource.id,
                    entitlement_error = %reason,
                    cooldown_seconds,
                    resets_at = %resets_at,
                    "Claude 订阅账号 entitlement/权限错误已转换为临时调度隔离回执"
                );
                Some(ResourceFeedback::AccountQuotaLimited { resets_at })
            }
            UpstreamFeedback::TemporarilyUnavailable { .. } => None,
        };
        Ok(command)
    }
}

fn safe_probe_error_type(body: &[u8]) -> String {
    serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|value| value.get("error")?.get("type")?.as_str().map(str::to_owned))
        .filter(|value| {
            value.len() <= 64
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        })
        .unwrap_or_else(|| "unknown_upstream_error".to_owned())
}

fn bad_response(error: AppError) -> MaintenanceFailure {
    MaintenanceFailure {
        kind: MaintenanceFailureKind::BadResponse,
        message: error.to_string(),
    }
}

pub fn next_token_refresh_at(
    state: &AppState,
    access_token_expires_at: DateTime<Utc>,
) -> DateTime<Utc> {
    access_token_expires_at
        - chrono::Duration::seconds(state.config().claude_token_refresh_ahead_seconds as i64)
}
