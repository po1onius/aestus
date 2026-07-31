//! GPT OAuth 账号的 Dashboard HTTP 接口、导入流程与额度查询。

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{delete, get, post, put},
};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};
use uuid::Uuid;

use crate::{
    err::{AdminResult, AppError, AppResult},
    http::dash::{
        auth as dash_auth,
        pagination::{ListPage, ListPageQuery},
    },
    provider::{
        credential::{ACCOUNT_STATUS_UNAUTHORIZED, ProviderAccount},
        gpt::{
            auth::{self, RefreshTokenGrant, RefreshedAuthToken},
            maintenance::{self as gpt_maintenance, GptMaintenance},
            model::{self as gpt_model, GptAccountSpecific},
            quota,
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

const MAX_REFRESH_TOKEN_BYTES: usize = 32 * 1024;
const MAX_CLIENT_ID_BYTES: usize = 512;
const MAX_ACCOUNT_ID_BYTES: usize = 512;
const MAX_CALLBACK_URL_BYTES: usize = 16 * 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateGptAccountRequest {
    refresh_token: Option<String>,
    client_id: Option<String>,
    chatgpt_account_id: Option<String>,
    #[serde(default, rename = "override")]
    override_: RequestOverride,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderGroupRequest {
    group_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateGptAccountEnabledRequest {
    enabled: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompleteOauthRequest {
    callback_url: String,
    #[serde(default, rename = "override")]
    override_: RequestOverride,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateRequestOverrideRequest {
    #[serde(rename = "override")]
    override_: RequestOverride,
}

#[derive(Debug, Serialize)]
struct CreateOauthAuthorizationResponse {
    authorization_url: String,
    redirect_uri: String,
    state: String,
    expires_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize)]
struct DeleteGptAccountResponse {
    id: Uuid,
}

#[derive(Debug, Serialize)]
struct GptAccountResponse {
    id: Uuid,
    account_id: Option<String>,
    client_id: String,
    email: Option<String>,
    plan_type: String,
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
        .route("/", get(list_gpt_accounts).post(create_gpt_account))
        .route("/oauth/authorize", post(create_oauth_authorization))
        .route("/oauth/callback", post(complete_oauth_callback))
        .route("/{id}", delete(delete_gpt_account))
        .route("/{id}/quota", post(refresh_gpt_account_quota))
        .route("/{id}/enabled", put(update_gpt_account_enabled))
        .route("/{id}/override", put(update_gpt_account_override))
        .route("/{id}/group", put(update_gpt_account_group))
}

async fn create_oauth_authorization(
    State(state): State<AppState>,
    _admin: dash_auth::AdminUser,
) -> AdminResult<Json<CreateOauthAuthorizationResponse>> {
    let authorization = auth::create_authorization(&state).await?;
    // OAuth 握手与账号最终归属解耦：Redis 只保存 PKCE 临时参数，不记录 Provider 分组。
    provider_oauth::create(
        &state,
        gpt_model::PROVIDER,
        &authorization.state,
        authorization.pkce_verifier,
        authorization.redirect_uri.clone(),
        authorization.expires_at,
    )
    .await?;

    Ok(Json(CreateOauthAuthorizationResponse {
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
) -> AdminResult<Json<GptAccountResponse>> {
    // override 属于纯本地输入，必须在消费一次性 OAuth state 和交换 code 之前完成校验。
    payload.override_.validate()?;
    let callback_url =
        normalize_required_limited(payload.callback_url, "callback_url", MAX_CALLBACK_URL_BYTES)?;
    let callback = auth::parse_callback_url(&callback_url)?;

    let session = provider_oauth::take(&state, gpt_model::PROVIDER, &callback.state)
        .await?
        .ok_or_else(|| AppError::BadRequest {
            message: "OAuth state 无效或已过期，请重新生成授权 URL".to_owned(),
        })?;

    let auth_token = auth::exchange_callback_code(
        &state,
        &session.redirect_uri,
        &session.pkce_verifier,
        &callback.code,
    )
    .await?;
    let account = persist_oauth_auth_token(&state, auth_token, payload.override_).await?;
    let snapshot = ProviderResourceService::<GptMaintenance>::new(&state)
        .sync_account(account)
        .await?;

    info!(
        gpt_account_id = %snapshot.account.id,
        oauth_state = %callback.state,
        "GPT OAuth callback 已完成，未分组账号已保存并进入统一 maintenance"
    );

    Ok(Json(GptAccountResponse::from_snapshot(snapshot)?))
}

async fn create_gpt_account(
    State(state): State<AppState>,
    _admin: dash_auth::AdminUser,
    Json(payload): Json<CreateGptAccountRequest>,
) -> AdminResult<Json<GptAccountResponse>> {
    payload.override_.validate()?;
    let refresh_token = normalize_optional_limited(
        payload.refresh_token,
        "refresh_token",
        MAX_REFRESH_TOKEN_BYTES,
    )?
    .ok_or_else(|| AppError::BadRequest {
        message: "refresh_token 不能为空".to_owned(),
    })?;
    let client_id =
        normalize_optional_limited(payload.client_id, "client_id", MAX_CLIENT_ID_BYTES)?
            .unwrap_or_else(|| auth::CODEX_OAUTH_CLIENT_ID.to_owned());
    let chatgpt_account_id = normalize_optional_limited(
        payload.chatgpt_account_id,
        "chatgpt_account_id",
        MAX_ACCOUNT_ID_BYTES,
    )?;
    let refresh_grant = match auth::refresh_token(&state, &refresh_token, &client_id).await {
        Ok(refresh_grant) => refresh_grant,
        Err(error) => {
            let kind = error.kind();
            warn!(
                failure_kind = kind.as_str(),
                client_id = %client_id,
                chatgpt_account_id = chatgpt_account_id.as_deref().unwrap_or("<missing>"),
                error = %error,
                "管理端 refresh_token 导入 GPT 账号失败"
            );
            return Err(map_refresh_token_import_error(error).into());
        }
    };
    let auth_token =
        auth_token_from_refresh_import(refresh_token, chatgpt_account_id, refresh_grant)?;
    let account =
        persist_imported_auth_token(&state, client_id, auth_token, payload.override_).await?;
    let snapshot = ProviderResourceService::<GptMaintenance>::new(&state)
        .sync_account(account)
        .await?;

    let specific = snapshot.account.parse_specific::<GptAccountSpecific>()?;

    info!(
        gpt_account_id = %snapshot.account.id,
        chatgpt_account_id = specific.chatgpt_account_id.as_deref().unwrap_or("<missing>"),
        client_id = %snapshot.account.client_id,
        "管理端通过 refresh_token 创建 GPT 账号成功"
    );

    Ok(Json(GptAccountResponse::from_snapshot(snapshot)?))
}

async fn persist_imported_auth_token(
    state: &AppState,
    client_id: String,
    auth_token: RefreshedAuthToken,
    request_override: RequestOverride,
) -> AppResult<ProviderAccount> {
    let plan_type = auth_token
        .plan_type
        .clone()
        .unwrap_or_else(|| gpt_model::PLAN_TYPE_UNKNOWN.to_owned());

    persist_auth_token_with_plan(state, client_id, auth_token, plan_type, request_override).await
}

fn auth_token_from_refresh_import(
    original_refresh_token: String,
    manual_chatgpt_account_id: Option<String>,
    refresh_grant: RefreshTokenGrant,
) -> AppResult<RefreshedAuthToken> {
    let access_token = refresh_grant
        .access_token
        .ok_or_else(|| AppError::BadRequest {
            message: "refresh_token 导入失败: refresh 响应缺少新的 access_token".to_owned(),
        })?;
    let access_token_expires_at =
        refresh_grant
            .access_token_expires_at
            .ok_or_else(|| AppError::BadRequest {
                message: "refresh_token 导入失败: access_token JWT 缺少 exp".to_owned(),
            })?;
    let chatgpt_account_id = refresh_grant
        .chatgpt_account_id
        .or(manual_chatgpt_account_id)
        .ok_or_else(|| AppError::BadRequest {
            message: "refresh_token 导入失败: id_token 和手动输入都未提供 chatgpt_account_id"
                .to_owned(),
        })?;

    Ok(RefreshedAuthToken {
        access_token,
        refresh_token: refresh_grant
            .refresh_token
            .unwrap_or(original_refresh_token),
        access_token_expires_at,
        chatgpt_account_id: Some(chatgpt_account_id),
        chatgpt_account_is_fedramp: refresh_grant.chatgpt_account_is_fedramp.unwrap_or(false),
        email: refresh_grant.email,
        plan_type: refresh_grant.plan_type,
    })
}

fn map_refresh_token_import_error(error: auth::TokenRefreshError) -> AppError {
    let kind = error.kind();
    let message = format!("refresh_token 导入失败: {error}");

    match kind {
        auth::TokenRefreshFailureKind::InvalidRefreshToken
        | auth::TokenRefreshFailureKind::BadResponse => AppError::BadRequest { message },
        auth::TokenRefreshFailureKind::RateLimited | auth::TokenRefreshFailureKind::Retryable => {
            AppError::GptUpstream { message }
        }
    }
}

async fn persist_oauth_auth_token(
    state: &AppState,
    auth_token: RefreshedAuthToken,
    request_override: RequestOverride,
) -> AppResult<ProviderAccount> {
    let plan_type = auth_token
        .plan_type
        .clone()
        .unwrap_or_else(|| gpt_model::PLAN_TYPE_UNKNOWN.to_owned());

    persist_auth_token_with_plan(
        state,
        auth::CODEX_OAUTH_CLIENT_ID.to_owned(),
        auth_token,
        plan_type,
        request_override,
    )
    .await
}

async fn persist_auth_token_with_plan(
    state: &AppState,
    client_id: String,
    auth_token: RefreshedAuthToken,
    plan_type: String,
    request_override: RequestOverride,
) -> AppResult<ProviderAccount> {
    let mut conn = state.db_conn().await?;

    // 管理端 OAuth 登录和 refresh_token 导入共享最终落库动作。plan_type 只来自
    // Codex id_token claims；缺失时写入 unknown，避免把未知套餐误标成 free。
    account::create_with_override(
        &mut conn,
        auth_token.chatgpt_account_id,
        auth_token.email,
        plan_type,
        auth_token.refresh_token,
        client_id,
        auth_token.access_token,
        gpt_maintenance::next_token_refresh_at_from_exp(state, auth_token.access_token_expires_at),
        auth_token.chatgpt_account_is_fedramp,
        request_override,
    )
    .await
}

async fn list_gpt_accounts(
    State(state): State<AppState>,
    _admin: dash_auth::AdminUser,
    Query(query): Query<ListPageQuery>,
) -> AdminResult<Json<ListPage<GptAccountResponse>>> {
    let page = query.normalize()?;
    let snapshots = ProviderResourceService::<GptMaintenance>::new(&state)
        .list_accounts(page.query_limit(), page.offset())
        .await?;
    let items = snapshots
        .into_iter()
        .map(GptAccountResponse::from_snapshot)
        .collect::<AppResult<Vec<_>>>()?;

    Ok(Json(page.finish(items)))
}

async fn update_gpt_account_enabled(
    State(state): State<AppState>,
    _admin: dash_auth::AdminUser,
    Path(id): Path<Uuid>,
    Json(payload): Json<UpdateGptAccountEnabledRequest>,
) -> AdminResult<Json<GptAccountResponse>> {
    let snapshot = ProviderResourceService::<GptMaintenance>::new(&state)
        .update_account_enabled(id, payload.enabled)
        .await?;

    Ok(Json(GptAccountResponse::from_snapshot(snapshot)?))
}

async fn update_gpt_account_override(
    State(state): State<AppState>,
    _admin: dash_auth::AdminUser,
    Path(id): Path<Uuid>,
    Json(payload): Json<UpdateRequestOverrideRequest>,
) -> AdminResult<Json<GptAccountResponse>> {
    payload.override_.validate()?;
    let snapshot = ProviderResourceService::<GptMaintenance>::new(&state)
        .update_account_override(id, payload.override_)
        .await?;

    info!(
        gpt_account_id = %snapshot.account.id,
        "管理端更新 GPT 账号请求 override 成功，runtime 已同步"
    );
    Ok(Json(GptAccountResponse::from_snapshot(snapshot)?))
}

async fn update_gpt_account_group(
    State(state): State<AppState>,
    _admin: dash_auth::AdminUser,
    Path(id): Path<Uuid>,
    Json(payload): Json<ProviderGroupRequest>,
) -> AdminResult<Json<GptAccountResponse>> {
    let snapshot = ProviderResourceService::<GptMaintenance>::new(&state)
        .update_account_group(id, payload.group_id)
        .await?;
    info!(
        gpt_account_id = %snapshot.account.id,
        provider_group_id = ?snapshot.account.group_id,
        "管理端调整 GPT 账号分组成功，runtime 已同步"
    );
    Ok(Json(GptAccountResponse::from_snapshot(snapshot)?))
}

async fn refresh_gpt_account_quota(
    State(state): State<AppState>,
    _admin: dash_auth::AdminUser,
    Path(id): Path<Uuid>,
) -> AdminResult<Json<quota::GptAccountQuotaResponse>> {
    let account = ProviderResourceService::<GptMaintenance>::new(&state)
        .find_account(id)
        .await?
        .ok_or_else(|| {
            warn!(gpt_account_id = %id, "管理端刷新 GPT 账号额度失败，账号不存在");
            AppError::BadRequest {
                message: format!("GPT 账号不存在: {id}"),
            }
        })?;

    let quota = quota::fetch_account_quota(&state, &account).await?;
    info!(
        gpt_account_id = %account.id,
        chatgpt_account_id = quota.chatgpt_account_id.as_deref().unwrap_or("<missing>"),
        snapshot_count = quota.snapshots.len(),
        "管理端 GPT 账号额度已刷新"
    );

    Ok(Json(quota))
}

async fn delete_gpt_account(
    State(state): State<AppState>,
    _admin: dash_auth::AdminUser,
    Path(id): Path<Uuid>,
) -> AdminResult<Json<DeleteGptAccountResponse>> {
    let service = ProviderResourceService::<GptMaintenance>::new(&state);
    let account_exists = service.find_account(id).await?.is_some();

    if !account_exists {
        warn!(gpt_account_id = %id, "管理端删除 GPT 账号失败，账号不存在");
        return Err(AppError::BadRequest {
            message: format!("GPT 账号不存在: {id}"),
        }
        .into());
    }

    let deleted = service.delete_account(id).await?;

    info!(
        gpt_account_id = %deleted.id,
        client_id = %deleted.client_id,
        "管理端已删除 GPT 账号，数据库凭证和 Redis runtime 均已清理"
    );

    Ok(Json(DeleteGptAccountResponse { id: deleted.id }))
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

impl GptAccountResponse {
    fn from_snapshot(snapshot: AccountSnapshot) -> AppResult<Self> {
        let AccountSnapshot {
            account,
            group,
            mut runtime,
        } = snapshot;
        let specific = account.parse_specific::<GptAccountSpecific>()?;
        let request_override = account.request_override()?;
        let now = chrono::Utc::now();
        // token/quota 时间属于 PostgreSQL 持久事实，不再重复写入 Redis runtime。管理端
        // 响应在这里合并两份视图，避免统一 runtime 精简后丢失维护时间展示。
        runtime.next_token_refresh_at = account.next_token_refresh_at;
        runtime.quota_resets_at = account.quota_resets_at;
        if group.is_none() {
            runtime.runtime_ready = false;
            runtime.token_usable = false;
            runtime.runtime_state = AccountRuntimeState::NotRuntime;
        } else if account
            .quota_resets_at
            .as_ref()
            .is_some_and(|quota_resets_at| *quota_resets_at > now)
        {
            runtime.runtime_ready = false;
            runtime.token_usable = false;
            runtime.runtime_state = AccountRuntimeState::QuotaLimited;
        } else if account.status == ACCOUNT_STATUS_UNAUTHORIZED && !runtime.runtime_ready {
            runtime.token_usable = false;
            runtime.runtime_state = AccountRuntimeState::TokenRefreshPending;
        }

        Ok(Self {
            id: account.id,
            account_id: specific.chatgpt_account_id,
            client_id: account.client_id.clone(),
            email: specific.email,
            plan_type: specific.plan_type,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn expires_at() -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::from_timestamp(1_893_456_000, 0).expect("valid timestamp")
    }

    fn refresh_grant() -> RefreshTokenGrant {
        RefreshTokenGrant {
            access_token: Some("access-token".to_owned()),
            refresh_token: None,
            access_token_expires_at: Some(expires_at()),
            chatgpt_account_id: None,
            chatgpt_account_is_fedramp: None,
            email: None,
            plan_type: None,
        }
    }

    #[test]
    fn refresh_import_uses_manual_account_id_when_id_token_omits_it() {
        let token = auth_token_from_refresh_import(
            "original-refresh".to_owned(),
            Some("manual-account".to_owned()),
            refresh_grant(),
        )
        .expect("manual account id should complete import");

        assert_eq!(token.access_token, "access-token");
        assert_eq!(token.refresh_token, "original-refresh");
        assert_eq!(token.chatgpt_account_id.as_deref(), Some("manual-account"));
        assert!(!token.chatgpt_account_is_fedramp);
        assert!(token.plan_type.is_none());
    }

    #[test]
    fn refresh_import_prefers_id_token_account_id() {
        let mut grant = refresh_grant();
        grant.chatgpt_account_id = Some("id-token-account".to_owned());

        let token = auth_token_from_refresh_import(
            "original-refresh".to_owned(),
            Some("manual-account".to_owned()),
            grant,
        )
        .expect("id_token account id should complete import");

        assert_eq!(
            token.chatgpt_account_id.as_deref(),
            Some("id-token-account")
        );
    }

    #[test]
    fn refresh_import_requires_access_token() {
        let mut grant = refresh_grant();
        grant.access_token = None;
        grant.access_token_expires_at = None;

        let result = auth_token_from_refresh_import(
            "original-refresh".to_owned(),
            Some("manual-account".to_owned()),
            grant,
        );

        assert!(matches!(result, Err(AppError::BadRequest { .. })));
    }

    #[test]
    fn refresh_import_requires_final_account_id() {
        let result =
            auth_token_from_refresh_import("original-refresh".to_owned(), None, refresh_grant());

        assert!(matches!(result, Err(AppError::BadRequest { .. })));
    }
}
