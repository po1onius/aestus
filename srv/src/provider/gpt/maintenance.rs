use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use tracing::{info, warn};
use uuid::Uuid;

use crate::{
    err::{AppError, AppResult},
    provider::{
        credential::{ProviderAccount, ProviderApiKey, serialize_specific},
        gpt::{
            auth::{self, TokenRefreshFailureKind},
            model::{GptAccountRequestContext, GptAccountSpecific, PROVIDER},
        },
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

const API_KEY_PROBE_MODEL: &str = "gpt-5.6-terra";
const API_KEY_PROBE_INPUT: &str = "Reply with OK.";

pub struct GptMaintenance;

impl MaintenanceProvider for GptMaintenance {
    const NAME: &'static str = PROVIDER;

    fn account_request_context(account: &ProviderAccount) -> AppResult<serde_json::Value> {
        let specific = account.parse_specific::<GptAccountSpecific>()?;
        serialize_request_context(&GptAccountRequestContext {
            chatgpt_account_id: specific.chatgpt_account_id,
            chatgpt_account_is_fedramp: specific.chatgpt_account_is_fedramp,
        })
    }

    async fn refresh_account<'a>(
        state: &'a AppState,
        account: &'a ProviderAccount,
    ) -> Result<RefreshedAccount, MaintenanceFailure> {
        let current = account
            .parse_specific::<GptAccountSpecific>()
            .map_err(bad_response)?;
        info!(
            gpt_account_id = %account.id,
            chatgpt_account_id = current.chatgpt_account_id.as_deref().unwrap_or("<missing>"),
            client_id = %account.client_id,
            "GPT maintenance 开始执行 provider 私有 refresh token 请求"
        );

        let grant = auth::refresh_token(state, &account.refresh_token, &account.client_id)
            .await
            .map_err(|error| MaintenanceFailure {
                kind: match error.kind() {
                    TokenRefreshFailureKind::InvalidRefreshToken => {
                        MaintenanceFailureKind::Unauthorized
                    }
                    TokenRefreshFailureKind::RateLimited => MaintenanceFailureKind::RateLimited,
                    TokenRefreshFailureKind::Retryable => MaintenanceFailureKind::Retryable,
                    TokenRefreshFailureKind::BadResponse => MaintenanceFailureKind::BadResponse,
                },
                message: error.to_string(),
            })?;
        let access_token = grant.access_token.ok_or_else(|| MaintenanceFailure {
            kind: MaintenanceFailureKind::BadResponse,
            message: "GPT refresh 响应缺少 access_token".to_owned(),
        })?;
        let expires_at = grant
            .access_token_expires_at
            .ok_or_else(|| MaintenanceFailure {
                kind: MaintenanceFailureKind::BadResponse,
                message: "GPT refresh 响应缺少 access token 过期时间".to_owned(),
            })?;
        let next_token_refresh_at = expires_at
            - chrono::Duration::seconds(state.config().gpt_token_refresh_ahead_seconds as i64);
        if next_token_refresh_at <= chrono::Utc::now() {
            return Err(MaintenanceFailure {
                kind: MaintenanceFailureKind::BadResponse,
                message: format!(
                    "GPT refresh 响应 access token 已进入刷新窗口: {next_token_refresh_at}"
                ),
            });
        }

        let specific = GptAccountSpecific {
            chatgpt_account_id: grant.chatgpt_account_id.or(current.chatgpt_account_id),
            email: grant.email.or(current.email),
            plan_type: grant.plan_type.unwrap_or(current.plan_type),
            chatgpt_account_is_fedramp: grant
                .chatgpt_account_is_fedramp
                .unwrap_or(current.chatgpt_account_is_fedramp),
        };
        Ok(RefreshedAccount {
            refresh_token: grant
                .refresh_token
                .unwrap_or_else(|| account.refresh_token.clone()),
            access_token,
            next_token_refresh_at,
            specific: serialize_specific(&specific).map_err(bad_response)?,
        })
    }

    async fn probe_api_key<'a>(
        state: &'a AppState,
        api_key: &'a ProviderApiKey,
    ) -> Result<(), MaintenanceFailure> {
        let url = build_official_url(
            &api_key.base_url,
            &state.config().gpt_upstream_responses_path,
            None,
        );
        info!(
            provider_api_key_id = %api_key.id,
            upstream_url = %url,
            probe_model = API_KEY_PROBE_MODEL,
            "GPT 官方 API Key 开始通过正式 Responses 请求探活"
        );
        let response = state
            .http_client()
            .post(&url)
            .bearer_auth(api_key.api_key.trim())
            .json(&serde_json::json!({
                "model": API_KEY_PROBE_MODEL,
                "input": API_KEY_PROBE_INPUT,
            }))
            .send()
            .await
            .map_err(|source| MaintenanceFailure {
                kind: MaintenanceFailureKind::Retryable,
                message: format!("GPT API Key Responses 探活请求失败: {source}"),
            })?;
        let status = response.status();
        let body = response
            .bytes()
            .await
            .map_err(|source| MaintenanceFailure {
                kind: MaintenanceFailureKind::Retryable,
                message: format!("读取 GPT API Key Responses 探活响应体失败: {source}"),
            })?;
        if status.is_success() {
            info!(
                provider_api_key_id = %api_key.id,
                upstream_status = status.as_u16(),
                response_bytes = body.len(),
                probe_model = API_KEY_PROBE_MODEL,
                "GPT 官方 API Key 正式 Responses 探活成功"
            );
            return Ok(());
        }

        let oauth_error = safe_probe_error_code(&body);
        let tracing_body = response_body_for_tracing(&body);
        warn!(
            provider_api_key_id = %api_key.id,
            upstream_status = status.as_u16(),
            response_bytes = body.len(),
            oauth_error,
            upstream_response_body_encoding = tracing_body.encoding(),
            upstream_response_body = %tracing_body.content(),
            "GPT 官方 API Key 探活失败，完整响应正文已写入 tracing，但不会写入 maintenance 状态"
        );
        Err(MaintenanceFailure {
            kind: if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
                MaintenanceFailureKind::Unauthorized
            } else if status == StatusCode::TOO_MANY_REQUESTS {
                MaintenanceFailureKind::RateLimited
            } else {
                MaintenanceFailureKind::Retryable
            },
            message: format!(
                "GPT API Key Responses 探活返回失败状态: status={status}, error={oauth_error}"
            ),
        })
    }

    fn account_refresh_retry_seconds(state: &AppState) -> u64 {
        state.config().gpt_token_refresh_retry_seconds
    }

    fn api_key_probe_interval_seconds(state: &AppState) -> u64 {
        state.config().gpt_upstream_api_key_probe_interval_seconds
    }

    fn feedback_command(
        state: &AppState,
        request_id: Uuid,
        resource: &UpstreamResource,
        feedback: UpstreamFeedback,
    ) -> AppResult<Option<ResourceFeedback>> {
        if resource.kind == UpstreamResourceKind::ApiKey {
            return Ok(Some(ResourceFeedback::ApiKeyError {
                reason: feedback.into_reason(),
            }));
        }

        let command = match feedback {
            UpstreamFeedback::Error { .. } => None,
            UpstreamFeedback::AuthenticationRejected { reason } => {
                Some(ResourceFeedback::AccountUnauthorized {
                    reason: format!(
                        "GPT 请求期上游拒绝认证: request_id={request_id}, reason={reason}"
                    ),
                })
            }
            UpstreamFeedback::RateLimited { resets_at, .. }
            | UpstreamFeedback::QuotaExhausted { resets_at, .. } => {
                Some(ResourceFeedback::AccountQuotaLimited {
                    resets_at: resets_at.unwrap_or_else(|| default_quota_reset(state)),
                })
            }
            UpstreamFeedback::TemporarilyUnavailable { .. }
            | UpstreamFeedback::EntitlementMissing { .. } => None,
        };
        Ok(command)
    }
}

fn safe_probe_error_code(body: &[u8]) -> String {
    serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|value| value.get("error")?.get("code")?.as_str().map(str::to_owned))
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

pub fn next_token_refresh_at_from_exp(
    state: &AppState,
    access_token_expires_at: DateTime<Utc>,
) -> DateTime<Utc> {
    access_token_expires_at
        - chrono::Duration::seconds(state.config().gpt_token_refresh_ahead_seconds as i64)
}

pub fn build_official_url(base_url: &str, path: &str, query: Option<&str>) -> String {
    let path = if path.starts_with('/') {
        path.to_owned()
    } else {
        format!("/{path}")
    };
    let mut url = format!("{}{}", base_url.trim().trim_end_matches('/'), path);
    if let Some(query) = query.filter(|query| !query.is_empty()) {
        url.push('?');
        url.push_str(query);
    }
    url
}

fn default_quota_reset(state: &AppState) -> DateTime<Utc> {
    Utc::now() + chrono::Duration::seconds(state.config().gpt_quota_recovery_seconds.max(1) as i64)
}
