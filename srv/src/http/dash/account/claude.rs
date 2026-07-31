//! Claude OAuth 账号的 Dashboard HTTP 接口与导入流程。

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{delete, get, post, put},
};
use serde::{Deserialize, Serialize};
use tracing::info;
use uuid::Uuid;

use crate::{
    err::{AdminResult, AppError, AppResult},
    http::dash::{
        auth as dash_auth,
        pagination::{ListPage, ListPageQuery},
    },
    provider::{
        claude::{
            auth,
            maintenance::{self as claude_maintenance, ClaudeMaintenance},
            model::{ClaudeAccountSpecific, ClaudeSubscriptionType, PROVIDER},
            sql::account,
        },
        group::ProviderGroup,
        oauth as provider_oauth,
        resource::RequestOverride,
        runtime::{AccountRuntimeState, AccountRuntimeView},
        service::{AccountSnapshot, ProviderResourceService},
    },
    state::AppState,
};

const MAX_AUTHORIZATION_RESULT_BYTES: usize = 16 * 1024;
const MAX_OAUTH_STATE_BYTES: usize = 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderGroupRequest {
    group_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompleteOauthRequest {
    authorization_result: String,
    state: Option<String>,
    #[serde(default, rename = "override")]
    override_: RequestOverride,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateEnabledRequest {
    enabled: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateOverrideRequest {
    #[serde(rename = "override")]
    override_: RequestOverride,
}

#[derive(Debug, Serialize)]
struct OauthAuthorizationResponse {
    authorization_url: String,
    redirect_uri: String,
    state: String,
    expires_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize)]
struct DeleteClaudeAccountResponse {
    id: Uuid,
}

#[derive(Debug, Serialize)]
struct ClaudeAccountResponse {
    id: Uuid,
    account_uuid: Option<String>,
    organization_uuid: Option<String>,
    email: Option<String>,
    display_name: Option<String>,
    subscription_type: Option<ClaudeSubscriptionType>,
    rate_limit_tier: Option<String>,
    has_extra_usage_enabled: Option<bool>,
    billing_type: Option<String>,
    account_created_at: Option<chrono::DateTime<chrono::Utc>>,
    subscription_created_at: Option<chrono::DateTime<chrono::Utc>>,
    client_id: String,
    scopes: Vec<String>,
    refresh_token_expires_at: Option<chrono::DateTime<chrono::Utc>>,
    quota_resets_at: Option<chrono::DateTime<chrono::Utc>>,
    enabled: bool,
    group: Option<ProviderGroup>,
    status: String,
    status_reason: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
    #[serde(rename = "override")]
    override_: RequestOverride,
    runtime: AccountRuntimeView,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list_accounts))
        .route("/oauth/authorize", post(create_oauth_authorization))
        .route("/oauth/callback", post(complete_oauth_callback))
        .route("/{id}", delete(delete_account))
        .route("/{id}/enabled", put(update_account_enabled))
        .route("/{id}/override", put(update_account_override))
        .route("/{id}/group", put(update_account_group))
}

async fn create_oauth_authorization(
    State(state): State<AppState>,
    _admin: dash_auth::AdminUser,
) -> AdminResult<Json<OauthAuthorizationResponse>> {
    let authorization = auth::create_authorization(&state)?;
    // OAuth 握手只在 Redis 保存 PKCE 临时参数；账号分组由 callback 创建请求单独决定。
    provider_oauth::create(
        &state,
        PROVIDER,
        &authorization.state,
        authorization.pkce_verifier,
        authorization.redirect_uri.clone(),
        authorization.expires_at,
    )
    .await?;

    Ok(Json(OauthAuthorizationResponse {
        authorization_url: authorization.authorization_url,
        redirect_uri: authorization.redirect_uri,
        state: authorization.state,
        expires_at: authorization.expires_at,
    }))
}

async fn complete_oauth_callback(
    State(state): State<AppState>,
    _admin: dash_auth::AdminUser,
    Json(payload): Json<CompleteOauthRequest>,
) -> AdminResult<Json<ClaudeAccountResponse>> {
    payload.override_.validate()?;
    let authorization_result = normalize_required_limited(
        payload.authorization_result,
        "authorization_result",
        MAX_AUTHORIZATION_RESULT_BYTES,
    )?;
    let oauth_state = normalize_optional_limited(payload.state, "state", MAX_OAUTH_STATE_BYTES)?;
    let authorization =
        auth::parse_authorization_result(&authorization_result, oauth_state.as_deref())?;

    let session = provider_oauth::take(&state, PROVIDER, &authorization.state)
        .await?
        .ok_or_else(|| AppError::BadRequest {
            message: "Claude OAuth state 无效或已过期，请重新生成授权链接".to_owned(),
        })?;

    let token = auth::exchange_code(
        &state,
        &session.redirect_uri,
        &session.pkce_verifier,
        &authorization.code,
        &authorization.state,
    )
    .await?;
    // Profile 是付费订阅准入和账号 UUID 去重的权威来源；校验完成前绝不持久化 token。
    let token = auth::validate_and_enrich_oauth_token(&state, token).await?;
    let mut conn = state.db_conn().await?;
    let account = account::create(
        &mut conn,
        token.refresh_token,
        token.access_token,
        claude_maintenance::next_token_refresh_at(&state, token.access_token_expires_at),
        token.specific,
        payload.override_,
    )
    .await?;
    drop(conn);
    let snapshot = ProviderResourceService::<ClaudeMaintenance>::new(&state)
        .sync_account(account)
        .await?;
    info!(
        claude_account_id = %snapshot.account.id,
        oauth_state = %authorization.state,
        "Claude OAuth callback 已完成，未分组账号已保存并进入统一 maintenance"
    );

    Ok(Json(ClaudeAccountResponse::from_snapshot(snapshot)?))
}

async fn list_accounts(
    State(state): State<AppState>,
    _admin: dash_auth::AdminUser,
    Query(query): Query<ListPageQuery>,
) -> AdminResult<Json<ListPage<ClaudeAccountResponse>>> {
    let page = query.normalize()?;
    let snapshots = ProviderResourceService::<ClaudeMaintenance>::new(&state)
        .list_accounts(page.query_limit(), page.offset())
        .await?;
    let items = snapshots
        .into_iter()
        .map(ClaudeAccountResponse::from_snapshot)
        .collect::<AppResult<Vec<_>>>()?;
    Ok(Json(page.finish(items)))
}

async fn update_account_enabled(
    State(state): State<AppState>,
    _admin: dash_auth::AdminUser,
    Path(id): Path<Uuid>,
    Json(payload): Json<UpdateEnabledRequest>,
) -> AdminResult<Json<ClaudeAccountResponse>> {
    let snapshot = ProviderResourceService::<ClaudeMaintenance>::new(&state)
        .update_account_enabled(id, payload.enabled)
        .await?;
    Ok(Json(ClaudeAccountResponse::from_snapshot(snapshot)?))
}

async fn update_account_override(
    State(state): State<AppState>,
    _admin: dash_auth::AdminUser,
    Path(id): Path<Uuid>,
    Json(payload): Json<UpdateOverrideRequest>,
) -> AdminResult<Json<ClaudeAccountResponse>> {
    payload.override_.validate()?;
    let snapshot = ProviderResourceService::<ClaudeMaintenance>::new(&state)
        .update_account_override(id, payload.override_)
        .await?;
    Ok(Json(ClaudeAccountResponse::from_snapshot(snapshot)?))
}

async fn update_account_group(
    State(state): State<AppState>,
    _admin: dash_auth::AdminUser,
    Path(id): Path<Uuid>,
    Json(payload): Json<ProviderGroupRequest>,
) -> AdminResult<Json<ClaudeAccountResponse>> {
    let snapshot = ProviderResourceService::<ClaudeMaintenance>::new(&state)
        .update_account_group(id, payload.group_id)
        .await?;
    info!(
        claude_account_id = %snapshot.account.id,
        provider_group_id = ?snapshot.account.group_id,
        "管理端调整 Claude 账号分组成功，runtime 已同步"
    );
    Ok(Json(ClaudeAccountResponse::from_snapshot(snapshot)?))
}

async fn delete_account(
    State(state): State<AppState>,
    _admin: dash_auth::AdminUser,
    Path(id): Path<Uuid>,
) -> AdminResult<Json<DeleteClaudeAccountResponse>> {
    let deleted = ProviderResourceService::<ClaudeMaintenance>::new(&state)
        .delete_account(id)
        .await?;
    info!(
        claude_account_id = %deleted.id,
        client_id = %deleted.client_id,
        "管理端已删除 Claude OAuth 账号和 Redis runtime"
    );
    Ok(Json(DeleteClaudeAccountResponse { id: deleted.id }))
}

impl ClaudeAccountResponse {
    fn from_snapshot(snapshot: AccountSnapshot) -> AppResult<Self> {
        let AccountSnapshot {
            account,
            group,
            mut runtime,
        } = snapshot;
        let specific = account.parse_specific::<ClaudeAccountSpecific>()?;
        let request_override = account.request_override()?;
        let now = chrono::Utc::now();
        runtime.next_token_refresh_at = account.next_token_refresh_at;
        runtime.quota_resets_at = account.quota_resets_at;
        if group.is_none() {
            runtime.runtime_ready = false;
            runtime.token_usable = false;
            runtime.runtime_state = AccountRuntimeState::NotRuntime;
        } else if account
            .quota_resets_at
            .is_some_and(|quota_resets_at| quota_resets_at > now)
        {
            runtime.runtime_ready = false;
            runtime.token_usable = false;
            runtime.runtime_state = AccountRuntimeState::QuotaLimited;
        } else if account.status == crate::provider::credential::ACCOUNT_STATUS_UNAUTHORIZED
            && !runtime.runtime_ready
        {
            runtime.token_usable = false;
            runtime.runtime_state = AccountRuntimeState::TokenRefreshPending;
        }
        Ok(Self {
            id: account.id,
            account_uuid: specific.account_uuid,
            organization_uuid: specific.organization_uuid,
            email: specific.email_address,
            display_name: specific.display_name,
            subscription_type: specific.subscription_type,
            rate_limit_tier: specific.rate_limit_tier,
            has_extra_usage_enabled: specific.has_extra_usage_enabled,
            billing_type: specific.billing_type,
            account_created_at: specific.account_created_at,
            subscription_created_at: specific.subscription_created_at,
            client_id: account.client_id.clone(),
            scopes: specific.scopes,
            refresh_token_expires_at: specific.refresh_token_expires_at,
            quota_resets_at: account.quota_resets_at,
            enabled: account.enabled,
            group,
            status: account.status.clone(),
            status_reason: account.status_reason.clone(),
            created_at: account.created_at,
            updated_at: account.updated_at,
            override_: request_override,
            runtime,
        })
    }
}

fn normalize_required_limited(
    value: String,
    field_name: &'static str,
    max_bytes: usize,
) -> AppResult<String> {
    let value = value.trim().to_owned();
    if value.is_empty() {
        return Err(AppError::BadRequest {
            message: format!("{field_name} 不能为空"),
        });
    }
    if value.len() > max_bytes {
        return Err(AppError::BadRequest {
            message: format!("{field_name} 不能超过 {max_bytes} 字节"),
        });
    }
    Ok(value)
}

fn normalize_optional_limited(
    value: Option<String>,
    field_name: &'static str,
    max_bytes: usize,
) -> AppResult<Option<String>> {
    let value = value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    if value.as_ref().is_some_and(|value| value.len() > max_bytes) {
        return Err(AppError::BadRequest {
            message: format!("{field_name} 不能超过 {max_bytes} 字节"),
        });
    }
    Ok(value)
}
