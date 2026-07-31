use std::time::Duration;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use rand::RngExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{info, warn};
use uuid::Uuid;

use crate::{
    err::{AppError, AppResult},
    provider::{
        claude::{
            OAUTH_BETA,
            model::{ClaudeAccountSpecific, ClaudeSubscriptionType},
        },
        response_logging::response_body_for_tracing,
    },
    state::AppState,
};

/// Claude Code 2.1.206 使用的公开 OAuth client ID。
pub const CLAUDE_OAUTH_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const CLAUDE_OAUTH_USER_AGENT: &str =
    concat!("aestus/", env!("CARGO_PKG_VERSION"), " claude-oauth");
const CLAUDE_OAUTH_PROFILE_PATH: &str = "/api/oauth/profile";
const CLAUDE_OAUTH_PROFILE_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_OAUTH_PROFILE_RESPONSE_BYTES: usize = 64 * 1024;
const MAX_OAUTH_PROFILE_TEXT_BYTES: usize = 1_024;

#[derive(Debug, Clone)]
pub struct ClaudeOauthAuthorize {
    pub authorization_url: String,
    pub redirect_uri: String,
    pub state: String,
    pub pkce_verifier: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct ClaudeOauthToken {
    pub access_token: String,
    pub refresh_token: String,
    pub access_token_expires_at: DateTime<Utc>,
    pub specific: ClaudeAccountSpecific,
}

#[derive(Debug, Clone)]
pub struct ClaudeRefreshGrant {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub access_token_expires_at: DateTime<Utc>,
    pub account_uuid: Option<String>,
    pub organization_uuid: Option<String>,
    pub email_address: Option<String>,
    pub scopes: Option<Vec<String>>,
    pub refresh_token_expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenFailureKind {
    InvalidGrant,
    RateLimited,
    Retryable,
    BadResponse,
}

#[derive(Debug, thiserror::Error)]
pub enum ClaudeTokenError {
    #[error("Claude OAuth token 请求失败: {message}")]
    Request { message: String },

    #[error("Claude OAuth token endpoint 返回失败: status={status}, error={oauth_error}")]
    UpstreamStatus {
        status: reqwest::StatusCode,
        oauth_error: String,
    },

    #[error("Claude OAuth token 响应无效: {message}")]
    BadResponse { message: String },
}

impl ClaudeTokenError {
    pub fn kind(&self) -> TokenFailureKind {
        match self {
            Self::Request { .. } => TokenFailureKind::Retryable,
            Self::BadResponse { .. } => TokenFailureKind::BadResponse,
            Self::UpstreamStatus {
                status,
                oauth_error,
            } => {
                let oauth_error = oauth_error.to_ascii_lowercase();
                if matches!(
                    *status,
                    reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN
                ) || oauth_error.contains("invalid_grant")
                    || oauth_error.contains("invalid_refresh")
                    || oauth_error.contains("refresh_token_expired")
                {
                    TokenFailureKind::InvalidGrant
                } else if *status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                    TokenFailureKind::RateLimited
                } else if *status == reqwest::StatusCode::REQUEST_TIMEOUT
                    || *status == reqwest::StatusCode::CONFLICT
                    || status.is_server_error()
                {
                    TokenFailureKind::Retryable
                } else {
                    TokenFailureKind::BadResponse
                }
            }
        }
    }
}

#[derive(Debug, Serialize)]
struct ExchangeCodeRequest<'a> {
    grant_type: &'static str,
    code: &'a str,
    redirect_uri: &'a str,
    client_id: &'a str,
    code_verifier: &'a str,
    state: &'a str,
}

#[derive(Debug, Serialize)]
struct RefreshTokenRequest<'a> {
    grant_type: &'static str,
    refresh_token: &'a str,
    client_id: &'a str,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    token_type: Option<String>,
    expires_in: Option<i64>,
    refresh_token_expires_in: Option<i64>,
    scope: Option<String>,
    account: Option<TokenAccount>,
    organization: Option<TokenOrganization>,
}

#[derive(Debug, Deserialize)]
struct TokenAccount {
    uuid: Option<String>,
    email_address: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TokenOrganization {
    uuid: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OauthErrorResponse {
    error: Option<String>,
}

/// `/api/oauth/profile` 的传输 DTO。只声明导入校验和管理端展示所需字段；上游新增字段会
/// 被 serde 忽略，避免 Profile 扩展时无故阻断账号导入。
#[derive(Debug, Deserialize)]
struct OauthProfileResponse {
    account: OauthProfileAccount,
    organization: OauthProfileOrganization,
}

#[derive(Debug, Deserialize)]
struct OauthProfileAccount {
    uuid: String,
    email: String,
    display_name: Option<String>,
    created_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
struct OauthProfileOrganization {
    uuid: String,
    organization_type: String,
    rate_limit_tier: Option<String>,
    has_extra_usage_enabled: Option<bool>,
    billing_type: Option<String>,
    subscription_created_at: Option<DateTime<Utc>>,
}

#[derive(Debug)]
struct ValidatedOauthProfile {
    account_uuid: String,
    organization_uuid: String,
    email_address: String,
    display_name: Option<String>,
    subscription_type: ClaudeSubscriptionType,
    rate_limit_tier: Option<String>,
    has_extra_usage_enabled: Option<bool>,
    billing_type: Option<String>,
    account_created_at: Option<DateTime<Utc>>,
    subscription_created_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct AuthorizationResult {
    pub code: String,
    pub state: String,
}

pub fn create_authorization(state: &AppState) -> AppResult<ClaudeOauthAuthorize> {
    let oauth_state = random_url_safe(32);
    let pkce_verifier = random_url_safe(64);
    let pkce_challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(pkce_verifier.as_bytes()));
    let redirect_uri = state.config().claude_oauth_redirect_uri.clone();
    let expires_at = Utc::now()
        + chrono::Duration::seconds(state.config().claude_oauth_session_ttl_seconds as i64);
    let mut url =
        reqwest::Url::parse(&state.config().claude_oauth_authorize_url).map_err(|source| {
            AppError::InvalidConfig {
                key: "AESTUS_CLAUDE_OAUTH_AUTHORIZE_URL",
                value: state.config().claude_oauth_authorize_url.clone(),
                source: Box::new(source),
            }
        })?;
    url.query_pairs_mut()
        .append_pair("code", "true")
        .append_pair("client_id", CLAUDE_OAUTH_CLIENT_ID)
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", &redirect_uri)
        .append_pair("scope", &state.config().claude_oauth_scope)
        .append_pair("code_challenge", &pkce_challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", &oauth_state);

    info!(
        oauth_state = %oauth_state,
        redirect_uri = %redirect_uri,
        expires_at = %expires_at,
        "Claude OAuth 授权 URL 已按 Claude Code PKCE 协议生成"
    );
    Ok(ClaudeOauthAuthorize {
        authorization_url: url.to_string(),
        redirect_uri,
        state: oauth_state,
        pkce_verifier,
        expires_at,
    })
}

/// 接受 Claude 授权页复制出的 `code#state`、完整 callback URL，或配合独立 state 的裸 code。
pub fn parse_authorization_result(
    input: &str,
    submitted_state: Option<&str>,
) -> AppResult<AuthorizationResult> {
    let input = input.trim();
    if input.is_empty() {
        return Err(AppError::BadRequest {
            message: "Claude OAuth authorization_result 不能为空".to_owned(),
        });
    }

    let parsed = if let Ok(url) = reqwest::Url::parse(input) {
        let code = url
            .query_pairs()
            .find_map(|(key, value)| (key == "code").then(|| value.into_owned()));
        let state = url
            .query_pairs()
            .find_map(|(key, value)| (key == "state").then(|| value.into_owned()));
        code.zip(state)
    } else if let Some((code, state)) = input.split_once('#') {
        Some((code.to_owned(), state.to_owned()))
    } else {
        submitted_state.map(|state| (input.to_owned(), state.to_owned()))
    };

    let Some((code, state)) = parsed else {
        return Err(AppError::BadRequest {
            message: "Claude OAuth 授权结果必须包含 code 和 state".to_owned(),
        });
    };
    let code = normalize_required(code, "Claude OAuth code 不能为空")?;
    let state = normalize_required(state, "Claude OAuth state 不能为空")?;
    if let Some(submitted_state) = submitted_state
        .map(str::trim)
        .filter(|value| !value.is_empty())
        && submitted_state != state
    {
        return Err(AppError::BadRequest {
            message: "Claude OAuth state 与当前授权会话不匹配".to_owned(),
        });
    }
    Ok(AuthorizationResult { code, state })
}

pub async fn exchange_code(
    state: &AppState,
    redirect_uri: &str,
    pkce_verifier: &str,
    code: &str,
    oauth_state: &str,
) -> AppResult<ClaudeOauthToken> {
    info!(
        token_endpoint = %state.config().claude_token_endpoint,
        oauth_state,
        "开始使用 Claude OAuth authorization code 交换 token"
    );
    let response = prepare_token_request(
        state
            .http_client()
            .post(&state.config().claude_token_endpoint),
    )
    .json(&ExchangeCodeRequest {
        grant_type: "authorization_code",
        code,
        redirect_uri,
        client_id: CLAUDE_OAUTH_CLIENT_ID,
        code_verifier: pkce_verifier,
        state: oauth_state,
    })
    .send()
    .await
    .map_err(|source| AppError::ProviderUpstream {
        provider: "claude".to_owned(),
        message: format!("OAuth code 交换请求失败: {source}"),
    })?;
    let payload = read_token_response(response)
        .await
        .map_err(map_exchange_error)?;
    let grant = token_grant_from_response(payload).map_err(map_exchange_error)?;
    let refresh_token = grant.refresh_token.ok_or_else(|| AppError::BadRequest {
        message: "Claude OAuth token 响应缺少 refresh_token".to_owned(),
    })?;
    let specific = ClaudeAccountSpecific {
        account_uuid: grant.account_uuid,
        organization_uuid: grant.organization_uuid,
        email_address: grant.email_address,
        display_name: None,
        subscription_type: None,
        rate_limit_tier: None,
        has_extra_usage_enabled: None,
        billing_type: None,
        account_created_at: None,
        subscription_created_at: None,
        scopes: grant.scopes.unwrap_or_else(|| configured_scopes(state)),
        refresh_token_expires_at: grant.refresh_token_expires_at,
    };
    info!(
        oauth_state,
        account_uuid = specific.account_uuid.as_deref().unwrap_or("<missing>"),
        organization_uuid = specific.organization_uuid.as_deref().unwrap_or("<missing>"),
        email = specific.email_address.as_deref().unwrap_or("<missing>"),
        scope_count = specific.scopes.len(),
        access_token_expires_at = %grant.access_token_expires_at,
        "Claude OAuth token 已解析，敏感 token 未写入日志"
    );
    Ok(ClaudeOauthToken {
        access_token: grant.access_token,
        refresh_token,
        access_token_expires_at: grant.access_token_expires_at,
        specific,
    })
}

/// 使用刚交换到的 Bearer token 读取官方 Profile，校验账号属于 Claude Code 支持的付费
/// 订阅，并把 Profile 身份覆盖到待持久化的 provider 私有字段中。
///
/// account UUID 是数据库去重键，因此 Profile 缺失、格式非法或与 token grant 明确冲突
/// 时必须终止导入，不能退回到 token 响应里的可选身份字段。
pub async fn validate_and_enrich_oauth_token(
    state: &AppState,
    mut token: ClaudeOauthToken,
) -> AppResult<ClaudeOauthToken> {
    let payload = fetch_oauth_profile(state, &token.access_token).await?;
    let profile = validate_oauth_profile(payload)?;

    if let Some(grant_account_uuid) = token.specific.account_uuid.as_deref() {
        let grant_account_uuid = canonical_oauth_uuid(grant_account_uuid, "token.account.uuid")?;
        if grant_account_uuid != profile.account_uuid {
            warn!("Claude OAuth token grant 与 Profile 返回了不同的 account UUID，已拒绝导入");
            return Err(AppError::ProviderUpstream {
                provider: "claude".to_owned(),
                message: "OAuth token grant 与 Profile 的账号身份不一致".to_owned(),
            });
        }
    }

    token.specific.account_uuid = Some(profile.account_uuid.clone());
    token.specific.organization_uuid = Some(profile.organization_uuid);
    token.specific.email_address = Some(profile.email_address);
    token.specific.display_name = profile.display_name;
    token.specific.subscription_type = Some(profile.subscription_type);
    token.specific.rate_limit_tier = profile.rate_limit_tier;
    token.specific.has_extra_usage_enabled = profile.has_extra_usage_enabled;
    token.specific.billing_type = profile.billing_type;
    token.specific.account_created_at = profile.account_created_at;
    token.specific.subscription_created_at = profile.subscription_created_at;

    info!(
        account_uuid = %profile.account_uuid,
        subscription_type = profile.subscription_type.as_str(),
        rate_limit_tier = token.specific.rate_limit_tier.as_deref().unwrap_or("<missing>"),
        has_extra_usage_enabled = ?token.specific.has_extra_usage_enabled,
        "Claude OAuth Profile 已校验，账号属于受支持的付费订阅"
    );
    Ok(token)
}

async fn fetch_oauth_profile(
    state: &AppState,
    access_token: &str,
) -> AppResult<OauthProfileResponse> {
    let endpoint = format!(
        "{}{}",
        state
            .config()
            .claude_upstream_base_url
            .trim_end_matches('/'),
        CLAUDE_OAUTH_PROFILE_PATH
    );
    info!(profile_endpoint = %endpoint, "开始请求 Claude OAuth Profile 以校验导入账号");
    let response = state
        .http_client()
        .get(&endpoint)
        .bearer_auth(access_token)
        // 与 Claude Code 的 Profile 请求保持一致；该 endpoint 不要求 OAuth beta header。
        .header(reqwest::header::ACCEPT, "application/json")
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header(reqwest::header::USER_AGENT, CLAUDE_OAUTH_USER_AGENT)
        .timeout(CLAUDE_OAUTH_PROFILE_TIMEOUT)
        .send()
        .await
        .map_err(|source| AppError::ProviderUpstream {
            provider: "claude".to_owned(),
            message: format!("OAuth Profile 请求失败: {source}"),
        })?;
    let status = response.status();
    let body = response
        .bytes()
        .await
        .map_err(|source| AppError::ProviderUpstream {
            provider: "claude".to_owned(),
            message: format!("读取 OAuth Profile 响应失败: {source}"),
        })?;

    if body.len() > MAX_OAUTH_PROFILE_RESPONSE_BYTES {
        warn!(
            status = status.as_u16(),
            response_bytes = body.len(),
            response_limit_bytes = MAX_OAUTH_PROFILE_RESPONSE_BYTES,
            "Claude OAuth Profile 响应超过大小限制"
        );
        return Err(AppError::ProviderUpstream {
            provider: "claude".to_owned(),
            message: format!(
                "OAuth Profile 响应超过 {} 字节限制",
                MAX_OAUTH_PROFILE_RESPONSE_BYTES
            ),
        });
    }

    if !status.is_success() {
        let tracing_body = response_body_for_tracing(&body);
        warn!(
            status = status.as_u16(),
            response_bytes = body.len(),
            upstream_response_body_encoding = tracing_body.encoding(),
            upstream_response_body = %tracing_body.content(),
            "Claude OAuth Profile endpoint 返回失败，完整响应正文已写入 tracing"
        );
        if status.is_client_error() && status != reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(AppError::BadRequest {
                message: format!("Claude OAuth Profile 校验失败: 上游返回 {status}"),
            });
        }
        return Err(AppError::ProviderUpstream {
            provider: "claude".to_owned(),
            message: format!("OAuth Profile endpoint 返回 {status}"),
        });
    }

    serde_json::from_slice(&body).map_err(|source| {
        let tracing_body = response_body_for_tracing(&body);
        warn!(
            status = status.as_u16(),
            response_bytes = body.len(),
            upstream_response_body_encoding = tracing_body.encoding(),
            upstream_response_body = %tracing_body.content(),
            error = %source,
            "Claude OAuth Profile 响应格式无效，完整响应正文已写入 tracing"
        );
        AppError::ProviderUpstream {
            provider: "claude".to_owned(),
            message: format!("OAuth Profile 返回非预期 JSON: {source}"),
        }
    })
}

fn validate_oauth_profile(payload: OauthProfileResponse) -> AppResult<ValidatedOauthProfile> {
    let account_uuid = canonical_oauth_uuid(&payload.account.uuid, "profile.account.uuid")?;
    let organization_uuid =
        canonical_oauth_uuid(&payload.organization.uuid, "profile.organization.uuid")?;
    let email_address = required_profile_text(payload.account.email, "account.email")?;
    let organization_type = required_profile_text(
        payload.organization.organization_type,
        "organization.organization_type",
    )?
    .to_ascii_lowercase();
    let subscription_type =
        ClaudeSubscriptionType::from_organization_type(&organization_type).ok_or_else(|| {
            warn!(
                organization_type,
                "Claude OAuth Profile 不是 Claude Code 支持的付费订阅"
            );
            AppError::BadRequest {
                message: format!(
                    "Claude OAuth 账号不是受支持的付费订阅（支持 Max、Pro、Team、Enterprise）: organization_type={organization_type}"
                ),
            }
        })?;

    Ok(ValidatedOauthProfile {
        account_uuid,
        organization_uuid,
        email_address,
        display_name: optional_profile_text(payload.account.display_name, "account.display_name")?,
        subscription_type,
        rate_limit_tier: optional_profile_text(
            payload.organization.rate_limit_tier,
            "organization.rate_limit_tier",
        )?,
        has_extra_usage_enabled: payload.organization.has_extra_usage_enabled,
        billing_type: optional_profile_text(
            payload.organization.billing_type,
            "organization.billing_type",
        )?,
        account_created_at: payload.account.created_at,
        subscription_created_at: payload.organization.subscription_created_at,
    })
}

fn canonical_oauth_uuid(value: &str, field: &'static str) -> AppResult<String> {
    let value = value.trim();
    let uuid = Uuid::parse_str(value).map_err(|source| AppError::ProviderUpstream {
        provider: "claude".to_owned(),
        message: format!("Claude OAuth 身份字段 {field} 不是有效 UUID: {source}"),
    })?;
    if uuid.is_nil() {
        return Err(AppError::ProviderUpstream {
            provider: "claude".to_owned(),
            message: format!("Claude OAuth 身份字段 {field} 不能是 nil UUID"),
        });
    }
    Ok(uuid.to_string())
}

fn required_profile_text(value: String, field: &'static str) -> AppResult<String> {
    optional_profile_text(Some(value), field)?.ok_or_else(|| AppError::ProviderUpstream {
        provider: "claude".to_owned(),
        message: format!("OAuth Profile 缺少字段 {field}"),
    })
}

fn optional_profile_text(value: Option<String>, field: &'static str) -> AppResult<Option<String>> {
    let value = value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    if value
        .as_ref()
        .is_some_and(|value| value.len() > MAX_OAUTH_PROFILE_TEXT_BYTES)
    {
        return Err(AppError::ProviderUpstream {
            provider: "claude".to_owned(),
            message: format!(
                "OAuth Profile 字段 {field} 超过 {MAX_OAUTH_PROFILE_TEXT_BYTES} 字节限制"
            ),
        });
    }
    Ok(value)
}

pub async fn refresh_token(
    state: &AppState,
    refresh_token: &str,
    client_id: &str,
) -> Result<ClaudeRefreshGrant, ClaudeTokenError> {
    info!(
        token_endpoint = %state.config().claude_token_endpoint,
        client_id,
        "开始执行 Claude provider 私有 refresh token 请求"
    );
    let response = prepare_token_request(
        state
            .http_client()
            .post(&state.config().claude_token_endpoint),
    )
    .json(&RefreshTokenRequest {
        grant_type: "refresh_token",
        refresh_token,
        client_id,
    })
    .send()
    .await
    .map_err(|source| ClaudeTokenError::Request {
        message: source.to_string(),
    })?;
    let payload = read_token_response(response).await?;
    token_grant_from_response(payload)
}

/// Claude OAuth token endpoint 使用 JSON，并要求 OAuth beta 参与后端路由。
///
/// User-Agent 只描述当前网关及版本，不包含账号、code 或 token 等敏感信息。显式设置这些
/// header 可以让 authorization-code 和 refresh-token 两条路径保持一致，也与 Anthropic
/// SDK 的 User OAuth refresh grant 行为一致。
fn prepare_token_request(request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    request
        .header(reqwest::header::ACCEPT, "application/json")
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header("anthropic-beta", OAUTH_BETA)
        .header(reqwest::header::USER_AGENT, CLAUDE_OAUTH_USER_AGENT)
}

fn configured_scopes(state: &AppState) -> Vec<String> {
    state
        .config()
        .claude_oauth_scope
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

async fn read_token_response(
    response: reqwest::Response,
) -> Result<TokenResponse, ClaudeTokenError> {
    let status = response.status();
    let body = response
        .bytes()
        .await
        .map_err(|source| ClaudeTokenError::Request {
            message: format!("读取 token 响应失败: {source}"),
        })?;
    if !status.is_success() {
        let oauth_error = safe_oauth_error(&body);
        let tracing_body = response_body_for_tracing(&body);
        warn!(
            status = status.as_u16(),
            oauth_error,
            response_bytes = body.len(),
            upstream_response_body_encoding = tracing_body.encoding(),
            upstream_response_body = %tracing_body.content(),
            "Claude OAuth token endpoint 返回失败，完整响应正文已写入 tracing"
        );
        return Err(ClaudeTokenError::UpstreamStatus {
            status,
            oauth_error,
        });
    }
    serde_json::from_slice(&body).map_err(|source| {
        let tracing_body = response_body_for_tracing(&body);
        warn!(
            status = status.as_u16(),
            response_bytes = body.len(),
            upstream_response_body_encoding = tracing_body.encoding(),
            upstream_response_body = %tracing_body.content(),
            error = %source,
            "Claude OAuth token endpoint 响应格式无效，完整响应正文已写入 tracing"
        );
        ClaudeTokenError::BadResponse {
            message: format!("token endpoint 返回非预期 JSON: {source}"),
        }
    })
}

fn token_grant_from_response(
    payload: TokenResponse,
) -> Result<ClaudeRefreshGrant, ClaudeTokenError> {
    if payload
        .token_type
        .as_deref()
        .is_some_and(|token_type| !token_type.eq_ignore_ascii_case("bearer"))
    {
        return Err(ClaudeTokenError::BadResponse {
            message: "token_type 不是 Bearer".to_owned(),
        });
    }
    let access_token = payload
        .access_token
        .and_then(normalize_optional)
        .ok_or_else(|| ClaudeTokenError::BadResponse {
            message: "响应缺少 access_token".to_owned(),
        })?;
    let expires_in = payload
        .expires_in
        .filter(|seconds| *seconds > 0)
        .ok_or_else(|| ClaudeTokenError::BadResponse {
            message: "响应缺少有效 expires_in".to_owned(),
        })?;
    let now = Utc::now();
    Ok(ClaudeRefreshGrant {
        access_token,
        refresh_token: payload.refresh_token.and_then(normalize_optional),
        access_token_expires_at: now + chrono::Duration::seconds(expires_in),
        account_uuid: payload
            .account
            .as_ref()
            .and_then(|account| account.uuid.clone().and_then(normalize_optional)),
        organization_uuid: payload
            .organization
            .and_then(|organization| organization.uuid.and_then(normalize_optional)),
        email_address: payload
            .account
            .and_then(|account| account.email_address.and_then(normalize_optional)),
        scopes: payload.scope.map(|scope| {
            scope
                .split_whitespace()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        }),
        refresh_token_expires_at: payload
            .refresh_token_expires_in
            .filter(|seconds| *seconds > 0)
            .map(|seconds| now + chrono::Duration::seconds(seconds)),
    })
}

fn safe_oauth_error(body: &[u8]) -> String {
    let Ok(error) = serde_json::from_slice::<OauthErrorResponse>(body) else {
        return "non_json_error".to_owned();
    };
    error
        .error
        .and_then(normalize_optional)
        .filter(|code| {
            code.len() <= 64
                && code
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        })
        .unwrap_or_else(|| "unknown_oauth_error".to_owned())
}

fn map_exchange_error(error: ClaudeTokenError) -> AppError {
    match error {
        ClaudeTokenError::UpstreamStatus {
            status,
            oauth_error,
        } if status.is_client_error() => AppError::BadRequest {
            message: format!("Claude OAuth code 交换失败: {oauth_error}"),
        },
        error => AppError::ProviderUpstream {
            provider: "claude".to_owned(),
            message: error.to_string(),
        },
    }
}

fn random_url_safe(byte_len: usize) -> String {
    let mut bytes = vec![0_u8; byte_len];
    rand::rng().fill(&mut bytes[..]);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn normalize_required(value: String, message: &'static str) -> AppResult<String> {
    normalize_optional(value).ok_or_else(|| AppError::BadRequest {
        message: message.to_owned(),
    })
}

fn normalize_optional(value: String) -> Option<String> {
    let value = value.trim().to_owned();
    (!value.is_empty()).then_some(value)
}
