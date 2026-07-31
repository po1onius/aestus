use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use rand::RngExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{info, warn};

use crate::{
    err::{AppError, AppResult},
    provider::{
        gpt::codex_http::request::{parse_id_token_claims, parse_jwt_expiration},
        response_logging::response_body_for_tracing,
    },
    state::AppState,
};

const AUTHORIZE_PATH: &str = "/oauth/authorize";
const CODEX_ORIGINATOR: &str = "codex_cli_rs";
/// OAuth 客户端 ID。
///
/// 与交互式 CLI 中使用的 client_id 保持一致；授权码交换和 refresh token 续期都必须使用
/// 同一个 client_id，否则导入成功后的账号可能无法稳定续期。
pub const CODEX_OAUTH_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const OAUTH_CLI_USER_AGENT: &str = "codex-cli/0.91.0";

#[derive(Debug, Clone)]
pub struct GptOauthAuthorize {
    pub authorization_url: String,
    pub redirect_uri: String,
    pub state: String,
    pub pkce_verifier: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
struct ExchangeCodeRequest<'a> {
    client_id: &'static str,
    grant_type: &'static str,
    code: &'a str,
    redirect_uri: &'a str,
    code_verifier: &'a str,
}

#[derive(Debug, Deserialize)]
struct ExchangeCodeResponse {
    id_token: String,
    access_token: String,
    refresh_token: String,
}

#[derive(Debug, Serialize)]
struct RefreshTokenRequest<'a> {
    client_id: &'a str,
    grant_type: &'static str,
    refresh_token: &'a str,
}

#[derive(Debug, Deserialize)]
struct RefreshTokenResponse {
    id_token: Option<String>,
    access_token: Option<String>,
    refresh_token: Option<String>,
}

/// OAuth 或 refresh_token 换出的可运行访问凭证。
///
/// 该结构只表达认证协议返回的结果，不写 PostgreSQL，不写 Redis，也不参与账号调度。
/// 调用方根据所在业务场景决定是否落库以及是否加载到 scheduler。
#[derive(Debug, Clone)]
pub struct RefreshedAuthToken {
    pub access_token: String,
    pub refresh_token: String,
    pub access_token_expires_at: DateTime<Utc>,
    pub chatgpt_account_id: Option<String>,
    pub chatgpt_account_is_fedramp: bool,
    pub email: Option<String>,
    pub plan_type: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RefreshTokenGrant {
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    pub access_token_expires_at: Option<DateTime<Utc>>,
    pub chatgpt_account_id: Option<String>,
    pub chatgpt_account_is_fedramp: Option<bool>,
    pub email: Option<String>,
    pub plan_type: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenRefreshFailureKind {
    InvalidRefreshToken,
    RateLimited,
    Retryable,
    BadResponse,
}

impl TokenRefreshFailureKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TokenRefreshFailureKind::InvalidRefreshToken => "invalid_refresh_token",
            TokenRefreshFailureKind::RateLimited => "rate_limited",
            TokenRefreshFailureKind::Retryable => "retryable",
            TokenRefreshFailureKind::BadResponse => "bad_response",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TokenRefreshError {
    #[error("刷新 token 请求失败: {message}")]
    Request { message: String },

    // body 仅用于内部错误分类，Display/日志/API 响应都不能回显可能携带凭证的上游正文。
    #[error("刷新 token 上游返回失败状态: {status}")]
    UpstreamStatus {
        status: reqwest::StatusCode,
        body: String,
    },

    #[error("刷新 token 响应格式无效: {message}")]
    BadResponse { message: String },
}

impl TokenRefreshError {
    pub fn kind(&self) -> TokenRefreshFailureKind {
        match self {
            TokenRefreshError::Request { .. } => TokenRefreshFailureKind::Retryable,
            TokenRefreshError::BadResponse { .. } => TokenRefreshFailureKind::BadResponse,
            TokenRefreshError::UpstreamStatus { status, body } => {
                if *status == reqwest::StatusCode::UNAUTHORIZED
                    || *status == reqwest::StatusCode::FORBIDDEN
                    || body_contains_invalid_refresh_token(body)
                {
                    TokenRefreshFailureKind::InvalidRefreshToken
                } else if *status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                    TokenRefreshFailureKind::RateLimited
                } else {
                    TokenRefreshFailureKind::Retryable
                }
            }
        }
    }
}

/// 创建 OAuth 授权 URL。
///
/// 这里按交互式 CLI 的方式实现：生成 state、PKCE verifier/challenge 和授权 URL，
/// 用户在浏览器授权后，把跳转到 localhost 的 callback URL 手动粘贴回前端。auth 模块
/// 只生成协议所需参数；调用方负责把 PKCE verifier 存入 Redis 短期会话。
pub async fn create_authorization(state: &AppState) -> AppResult<GptOauthAuthorize> {
    let oauth_state = random_url_safe(32);
    let pkce_verifier = random_url_safe(64);
    let pkce_challenge = pkce_s256_challenge(&pkce_verifier);
    let expires_at =
        Utc::now() + chrono::Duration::seconds(state.config().gpt_oauth_session_ttl_seconds as i64);
    let redirect_uri = state.config().gpt_oauth_redirect_uri.clone();

    let authorization_url = build_authorization_url(
        &state.config().gpt_oauth_issuer,
        &redirect_uri,
        &state.config().gpt_oauth_scope,
        &oauth_state,
        &pkce_challenge,
    )?;

    info!(
        oauth_state = %oauth_state,
        expires_at = %expires_at,
        "GPT OAuth 授权 URL 已生成"
    );

    Ok(GptOauthAuthorize {
        authorization_url,
        redirect_uri,
        state: oauth_state,
        pkce_verifier,
        expires_at,
    })
}

/// 使用裸 refresh_token 换取 access token。
pub async fn refresh_token(
    state: &AppState,
    refresh_token: &str,
    client_id: &str,
) -> Result<RefreshTokenGrant, TokenRefreshError> {
    let client_id = normalize_optional_ref(Some(client_id)).unwrap_or(CODEX_OAUTH_CLIENT_ID);
    info!(
        token_endpoint = %state.config().gpt_token_endpoint,
        client_id = %client_id,
        "开始使用 refresh_token 刷新 GPT token"
    );

    let response = state
        .http_client()
        .post(&state.config().gpt_token_endpoint)
        .header(reqwest::header::USER_AGENT, OAUTH_CLI_USER_AGENT)
        // Codex refresh token flow 使用 JSON body；响应字段全部是可选覆盖项。
        .json(&RefreshTokenRequest {
            client_id,
            grant_type: "refresh_token",
            refresh_token,
        });
    let response = response
        .send()
        .await
        .map_err(|source| TokenRefreshError::Request {
            message: source.to_string(),
        })?;

    let status = response.status();
    let body = response
        .bytes()
        .await
        .map_err(|source| TokenRefreshError::Request {
            message: source.to_string(),
        })?;

    if !status.is_success() {
        let tracing_body = response_body_for_tracing(&body);
        warn!(
            token_endpoint = %state.config().gpt_token_endpoint,
            upstream_status = status.as_u16(),
            response_bytes = body.len(),
            upstream_response_body_encoding = tracing_body.encoding(),
            upstream_response_body = %tracing_body.content(),
            "GPT refresh token 请求收到失败响应，完整响应正文已写入 tracing"
        );
        return Err(TokenRefreshError::UpstreamStatus {
            status,
            body: truncate_for_status_reason(&String::from_utf8_lossy(&body)),
        });
    }

    let payload = serde_json::from_slice::<RefreshTokenResponse>(&body).map_err(|source| {
        let tracing_body = response_body_for_tracing(&body);
        warn!(
            token_endpoint = %state.config().gpt_token_endpoint,
            upstream_status = status.as_u16(),
            response_bytes = body.len(),
            upstream_response_body_encoding = tracing_body.encoding(),
            upstream_response_body = %tracing_body.content(),
            error = %source,
            "GPT refresh token 响应格式无效，完整响应正文已写入 tracing"
        );
        TokenRefreshError::BadResponse {
            message: source.to_string(),
        }
    })?;

    refresh_grant_from_response(payload)
}

#[derive(Debug, Clone)]
pub struct CallbackParams {
    pub code: String,
    pub state: String,
}

pub fn parse_callback_url(callback_url: &str) -> AppResult<CallbackParams> {
    let url = reqwest::Url::parse(callback_url).map_err(|source| AppError::BadRequest {
        message: format!("callback_url 不是合法 URL: {source}"),
    })?;

    let mut code = None;
    let mut state = None;
    let mut error = None;
    let mut error_description = None;

    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "code" => code = Some(value.into_owned()),
            "state" => state = Some(value.into_owned()),
            "error" => error = Some(value.into_owned()),
            "error_description" => error_description = Some(value.into_owned()),
            _ => {}
        }
    }

    if let Some(error) = error {
        return Err(AppError::BadRequest {
            message: format!(
                "OAuth 授权失败: {error}{}",
                error_description
                    .map(|description| format!(", {description}"))
                    .unwrap_or_default()
            ),
        });
    }

    let code = normalize_required(code, "callback_url 缺少 code 参数")?;
    let state = normalize_required(state, "callback_url 缺少 state 参数")?;

    Ok(CallbackParams { code, state })
}

/// 使用 OAuth callback code 和 PKCE verifier 换取账号授权 token。
///
/// 该函数只负责和 OAuth token endpoint 交互以及解析认证响应，不负责消费/保存
/// OAuth 临时会话，也不负责账号落库。
pub async fn exchange_callback_code(
    state: &AppState,
    redirect_uri: &str,
    pkce_verifier: &str,
    code: &str,
) -> AppResult<RefreshedAuthToken> {
    info!(
        token_endpoint = %state.config().gpt_token_endpoint,
        "开始使用 OAuth callback code 交换 GPT token"
    );

    let request = state
        .http_client()
        .post(&state.config().gpt_token_endpoint)
        // 交互式 CLI token 交换使用 application/x-www-form-urlencoded。
        // code_verifier 只随这次服务端请求发送，避免暴露到前端页面和日志。
        .form(&ExchangeCodeRequest {
            client_id: CODEX_OAUTH_CLIENT_ID,
            grant_type: "authorization_code",
            code,
            redirect_uri,
            code_verifier: pkce_verifier,
        });
    let response = request
        .send()
        .await
        .map_err(|source| AppError::GptUpstream {
            message: format!("OAuth code 换 token 请求失败: {source}"),
        })?;

    let status = response.status();
    let body = response
        .bytes()
        .await
        .map_err(|source| AppError::GptUpstream {
            message: format!("OAuth code 换 token 响应读取失败: {source}"),
        })?;

    if !status.is_success() {
        let oauth_error = safe_oauth_error(&body);
        let tracing_body = response_body_for_tracing(&body);
        warn!(
            token_endpoint = %state.config().gpt_token_endpoint,
            upstream_status = status.as_u16(),
            oauth_error,
            response_bytes = body.len(),
            upstream_response_body_encoding = tracing_body.encoding(),
            upstream_response_body = %tracing_body.content(),
            "GPT OAuth code 换 token 失败，完整上游响应正文已写入 tracing"
        );
        return Err(AppError::BadRequest {
            message: format!(
                "OAuth code 换 token 失败: status={}, error={oauth_error}",
                status.as_u16(),
            ),
        });
    }

    let payload = serde_json::from_slice::<ExchangeCodeResponse>(&body).map_err(|source| {
        let tracing_body = response_body_for_tracing(&body);
        warn!(
            token_endpoint = %state.config().gpt_token_endpoint,
            upstream_status = status.as_u16(),
            response_bytes = body.len(),
            upstream_response_body_encoding = tracing_body.encoding(),
            upstream_response_body = %tracing_body.content(),
            error = %source,
            "GPT OAuth code 换 token 响应格式无效，完整响应正文已写入 tracing"
        );
        AppError::BadRequest {
            message: format!("OAuth token 响应格式无效: {source}"),
        }
    })?;

    refreshed_token_from_exchange_response(payload)
}

fn refreshed_token_from_exchange_response(
    payload: ExchangeCodeResponse,
) -> AppResult<RefreshedAuthToken> {
    let refresh_token =
        normalize_required(Some(payload.refresh_token), "OAuth 响应缺少 refresh_token")?;
    let access_token =
        normalize_required(Some(payload.access_token), "OAuth 响应缺少 access_token")?;
    let id_token = normalize_required(Some(payload.id_token), "OAuth 响应缺少 id_token")?;
    let claims = parse_id_token_claims(&id_token).map_err(|message| AppError::BadRequest {
        message: format!("OAuth id_token 无法解析: {message}"),
    })?;
    let chatgpt_account_id = claims.chatgpt_account_id.clone();
    let chatgpt_account_is_fedramp = claims.chatgpt_account_is_fedramp;
    let email = claims.email.clone();
    let plan_type = claims.chatgpt_plan_type.clone();
    let access_token_expires_at = parse_jwt_expiration(&access_token)
        .map_err(|message| AppError::BadRequest {
            message: format!("OAuth access_token 无法解析 exp: {message}"),
        })?
        .ok_or_else(|| AppError::BadRequest {
            message: "OAuth access_token JWT 缺少 exp，无法计算过期时间".to_owned(),
        })?;

    info!(
        chatgpt_account_id = chatgpt_account_id.as_deref().unwrap_or("<missing>"),
        email = email.as_deref().unwrap_or("<missing>"),
        plan_type = plan_type.as_deref().unwrap_or("<unknown>"),
        chatgpt_user_id = claims.chatgpt_user_id.as_deref().unwrap_or("<missing>"),
        access_token_expires_at = %access_token_expires_at.to_rfc3339(),
        chatgpt_account_is_fedramp,
        "OAuth token 响应已按 Codex id_token/access_token claims 解析"
    );

    Ok(RefreshedAuthToken {
        access_token,
        refresh_token,
        access_token_expires_at,
        chatgpt_account_id,
        chatgpt_account_is_fedramp,
        email,
        plan_type,
    })
}

fn refresh_grant_from_response(
    payload: RefreshTokenResponse,
) -> Result<RefreshTokenGrant, TokenRefreshError> {
    let access_token = normalize_optional_ref(payload.access_token.as_deref()).map(str::to_owned);
    let access_token_expires_at = access_token
        .as_deref()
        .map(parse_refresh_access_token_expiration)
        .transpose()?;
    let refresh_token = normalize_optional_ref(payload.refresh_token.as_deref()).map(str::to_owned);
    let claims = payload
        .id_token
        .as_deref()
        .and_then(|id_token| normalize_optional_ref(Some(id_token)))
        .map(parse_refresh_id_token_claims)
        .transpose()?;

    let (chatgpt_account_id, chatgpt_account_is_fedramp, email, plan_type, chatgpt_user_id) =
        match claims {
            Some(claims) => (
                claims.chatgpt_account_id,
                Some(claims.chatgpt_account_is_fedramp),
                claims.email,
                claims.chatgpt_plan_type,
                claims.chatgpt_user_id,
            ),
            None => (None, None, None, None, None),
        };

    info!(
        has_access_token = access_token.is_some(),
        has_refresh_token = refresh_token.is_some(),
        has_id_token = chatgpt_account_id.is_some()
            || email.is_some()
            || plan_type.is_some()
            || chatgpt_user_id.is_some()
            || chatgpt_account_is_fedramp.is_some(),
        chatgpt_account_id = chatgpt_account_id.as_deref().unwrap_or("<missing>"),
        email = email.as_deref().unwrap_or("<missing>"),
        plan_type = plan_type.as_deref().unwrap_or("<missing>"),
        chatgpt_user_id = chatgpt_user_id.as_deref().unwrap_or("<missing>"),
        "refresh_token 响应已按 Codex 可选字段模型解析"
    );

    Ok(RefreshTokenGrant {
        access_token,
        refresh_token,
        access_token_expires_at,
        chatgpt_account_id,
        chatgpt_account_is_fedramp,
        email,
        plan_type,
    })
}

fn parse_refresh_access_token_expiration(
    access_token: &str,
) -> Result<DateTime<Utc>, TokenRefreshError> {
    parse_jwt_expiration(access_token)
        .map_err(|message| TokenRefreshError::BadResponse {
            message: format!("refresh 响应 access_token 无法解析 exp: {message}"),
        })?
        .ok_or_else(|| TokenRefreshError::BadResponse {
            message: "refresh 响应 access_token JWT 缺少 exp，无法计算过期时间".to_owned(),
        })
}

fn parse_refresh_id_token_claims(
    id_token: &str,
) -> Result<crate::provider::gpt::codex_http::request::CodexIdTokenClaims, TokenRefreshError> {
    parse_id_token_claims(id_token).map_err(|message| TokenRefreshError::BadResponse {
        message: format!("refresh 响应 id_token 无法解析: {message}"),
    })
}

fn build_authorization_url(
    issuer: &str,
    redirect_uri: &str,
    scope: &str,
    oauth_state: &str,
    pkce_challenge: &str,
) -> AppResult<String> {
    let mut url = reqwest::Url::parse(&format!(
        "{}{}",
        issuer.trim_end_matches('/'),
        AUTHORIZE_PATH
    ))
    .map_err(|source| AppError::InvalidConfig {
        key: "AESTUS_GPT_OAUTH_ISSUER",
        value: issuer.to_owned(),
        source: Box::new(source),
    })?;
    url.query_pairs_mut()
        .append_pair("client_id", CODEX_OAUTH_CLIENT_ID)
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("scope", scope)
        .append_pair("state", oauth_state)
        .append_pair("code_challenge", pkce_challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("id_token_add_organizations", "true")
        .append_pair("codex_cli_simplified_flow", "true")
        .append_pair("originator", CODEX_ORIGINATOR);

    Ok(url.to_string())
}

fn pkce_s256_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn random_url_safe(byte_len: usize) -> String {
    let mut bytes = vec![0_u8; byte_len];
    rand::rng().fill(&mut bytes[..]);
    hex::encode(bytes)
}

fn normalize_required(value: Option<String>, message: &'static str) -> AppResult<String> {
    value
        .and_then(normalize_optional)
        .ok_or_else(|| AppError::BadRequest {
            message: message.to_owned(),
        })
}

fn normalize_optional(value: String) -> Option<String> {
    let value = value.trim().to_owned();
    (!value.is_empty()).then_some(value)
}

fn normalize_optional_ref(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn body_contains_invalid_refresh_token(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    lower.contains("invalid_grant")
        || lower.contains("invalid_refresh_token")
        || lower.contains("refresh_token_expired")
        || lower.contains("refresh_token_reused")
        || lower.contains("refresh_token_invalidated")
        || lower.contains("invalid refresh")
        || lower.contains("expired refresh")
        || lower.contains("unauthorized")
}

fn safe_oauth_error(body: &[u8]) -> String {
    serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|value| value.get("error")?.as_str().map(str::to_owned))
        .filter(|value| {
            value.len() <= 64
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        })
        .unwrap_or_else(|| "unknown_oauth_error".to_owned())
}

fn truncate_for_status_reason(value: &str) -> String {
    const MAX_STATUS_REASON_CHARS: usize = 2_048;

    value.chars().take(MAX_STATUS_REASON_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn test_jwt(payload: serde_json::Value) -> String {
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"none"}"#);
        let payload = URL_SAFE_NO_PAD.encode(payload.to_string());
        format!("{header}.{payload}.signature")
    }

    #[test]
    fn refresh_grant_parses_codex_optional_tokens_and_id_claims() {
        let access_exp = 1_893_456_000_i64;
        let access_token = test_jwt(json!({ "exp": access_exp }));
        let id_token = test_jwt(json!({
            "email": "user@example.com",
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "acc_123",
                "chatgpt_account_is_fedramp": true,
                "chatgpt_plan_type": "pro",
                "chatgpt_user_id": "user_123"
            }
        }));

        let grant = refresh_grant_from_response(RefreshTokenResponse {
            id_token: Some(id_token),
            access_token: Some(access_token.clone()),
            refresh_token: Some("next-refresh".to_owned()),
        })
        .expect("refresh response should parse");

        assert_eq!(grant.access_token.as_deref(), Some(access_token.as_str()));
        assert_eq!(grant.refresh_token.as_deref(), Some("next-refresh"));
        assert_eq!(
            grant.access_token_expires_at.map(|value| value.timestamp()),
            Some(access_exp)
        );
        assert_eq!(grant.chatgpt_account_id.as_deref(), Some("acc_123"));
        assert_eq!(grant.chatgpt_account_is_fedramp, Some(true));
        assert_eq!(grant.email.as_deref(), Some("user@example.com"));
        assert_eq!(grant.plan_type.as_deref(), Some("pro"));
    }

    #[test]
    fn refresh_grant_without_id_token_keeps_identity_empty() {
        let access_token = test_jwt(json!({ "exp": 1_893_456_000_i64 }));

        let grant = refresh_grant_from_response(RefreshTokenResponse {
            id_token: None,
            access_token: Some(access_token),
            refresh_token: None,
        })
        .expect("refresh response should parse without id_token");

        assert!(grant.chatgpt_account_id.is_none());
        assert!(grant.chatgpt_account_is_fedramp.is_none());
        assert!(grant.email.is_none());
        assert!(grant.plan_type.is_none());
    }

    #[test]
    fn refresh_grant_does_not_require_access_token() {
        let id_token = test_jwt(json!({
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "acc_123"
            }
        }));

        let grant = refresh_grant_from_response(RefreshTokenResponse {
            id_token: Some(id_token),
            access_token: None,
            refresh_token: Some("next-refresh".to_owned()),
        })
        .expect("Codex refresh response may omit access_token");

        assert!(grant.access_token.is_none());
        assert!(grant.access_token_expires_at.is_none());
        assert_eq!(grant.refresh_token.as_deref(), Some("next-refresh"));
        assert_eq!(grant.chatgpt_account_id.as_deref(), Some("acc_123"));
    }
}
