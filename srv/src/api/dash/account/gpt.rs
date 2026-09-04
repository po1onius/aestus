//! GPT OAuth 账号的 Dashboard HTTP 接口、导入流程与额度查询。

use std::collections::HashSet;

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{delete, get, post, put},
};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};
use uuid::Uuid;

use crate::{
    api::dash::{
        auth as dash_auth,
        pagination::{ListPage, ListPageQuery},
    },
    err::{AdminResult, AppError, AppResult},
    provider::{
        credential::{ACCOUNT_STATUS_UNAUTHORIZED, ProviderAccount},
        gpt::{
            auth::{self, RefreshTokenGrant, RefreshedAuthToken},
            maintenance::{self as gpt_maintenance, GptMaintenance},
            model::{self as gpt_model, GptAccountSpecific},
            quota, rate_limit_reset,
            sql::account,
        },
        group::ProviderGroup,
        oauth as provider_oauth,
        resource::RequestOverride,
        runtime::{AccountRuntimeState, AccountRuntimeView},
        service::{AccountSnapshot, ProviderResourceService},
    },
    state::AppState,
    user::group_access::{self, GroupPermission},
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
    override_: Option<RequestOverride>,
    runtime: AccountRuntimeView,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list_gpt_accounts).post(create_gpt_account))
        .route("/oauth/authorize", post(create_oauth_authorization))
        .route("/oauth/callback", post(complete_oauth_callback))
        .route("/{id}", delete(delete_gpt_account))
        .route("/{id}/quota", post(refresh_gpt_account_quota))
        .route(
            "/{id}/rate-limit-reset-credits",
            get(list_gpt_account_rate_limit_reset_credits),
        )
        .route(
            "/{id}/rate-limit-reset-credits/consume",
            post(consume_gpt_account_rate_limit_reset_credit),
        )
        .route("/{id}/enabled", put(update_gpt_account_enabled))
        .route("/{id}/override", put(update_gpt_account_override))
        .route("/{id}/group", put(update_gpt_account_group))
}

async fn create_oauth_authorization(
    State(state): State<AppState>,
    dash_auth::AdminUser(owner): dash_auth::AdminUser,
) -> AdminResult<Json<CreateOauthAuthorizationResponse>> {
    let tenant_id = owner.tenant_id.ok_or(AppError::Forbidden)?;
    let authorization = auth::create_authorization(&state).await?;
    // OAuth 握手与账号最终归属解耦：Redis 只保存 PKCE 临时参数，不记录 Provider 分组。
    provider_oauth::create(
        &state,
        gpt_model::PROVIDER,
        tenant_id,
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
    dash_auth::AdminUser(owner): dash_auth::AdminUser,
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
    let tenant_id = owner.tenant_id.ok_or(AppError::Forbidden)?;
    if session.tenant_id != tenant_id {
        warn!(owner_user_id = %owner.id, owner_tenant_id = %tenant_id, oauth_tenant_id = %session.tenant_id, "GPT OAuth 会话租户与当前 owner 不一致，拒绝消费");
        return Err(AppError::Forbidden);
    }

    let auth_token = auth::exchange_callback_code(
        &state,
        &session.redirect_uri,
        &session.pkce_verifier,
        &callback.code,
    )
    .await?;
    let account =
        persist_oauth_auth_token(&state, tenant_id, auth_token, payload.override_).await?;
    let snapshot = ProviderResourceService::<GptMaintenance>::new(&state)
        .sync_account(account)
        .await?;

    info!(
        gpt_account_id = %snapshot.account.id,
        oauth_state = %callback.state,
        "GPT OAuth callback 已完成，未分组账号已保存并进入统一 maintenance"
    );

    Ok(Json(GptAccountResponse::from_snapshot(snapshot, true)?))
}

async fn create_gpt_account(
    State(state): State<AppState>,
    dash_auth::AdminUser(owner): dash_auth::AdminUser,
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
            return Err(map_refresh_token_import_error(error));
        }
    };
    let auth_token =
        auth_token_from_refresh_import(refresh_token, chatgpt_account_id, refresh_grant)?;
    let tenant_id = owner.tenant_id.ok_or(AppError::Forbidden)?;
    let account =
        persist_imported_auth_token(&state, tenant_id, client_id, auth_token, payload.override_)
            .await?;
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

    Ok(Json(GptAccountResponse::from_snapshot(snapshot, true)?))
}

async fn persist_imported_auth_token(
    state: &AppState,
    tenant_id: Uuid,
    client_id: String,
    auth_token: RefreshedAuthToken,
    request_override: RequestOverride,
) -> AppResult<ProviderAccount> {
    let plan_type = auth_token
        .plan_type
        .clone()
        .unwrap_or_else(|| gpt_model::PLAN_TYPE_UNKNOWN.to_owned());

    persist_auth_token_with_plan(
        state,
        tenant_id,
        client_id,
        auth_token,
        plan_type,
        request_override,
    )
    .await
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
            AppError::ProviderUpstream {
                provider: gpt_model::PROVIDER.to_owned(),
                message,
            }
        }
    }
}

async fn persist_oauth_auth_token(
    state: &AppState,
    tenant_id: Uuid,
    auth_token: RefreshedAuthToken,
    request_override: RequestOverride,
) -> AppResult<ProviderAccount> {
    let plan_type = auth_token
        .plan_type
        .clone()
        .unwrap_or_else(|| gpt_model::PLAN_TYPE_UNKNOWN.to_owned());

    persist_auth_token_with_plan(
        state,
        tenant_id,
        auth::CODEX_OAUTH_CLIENT_ID.to_owned(),
        auth_token,
        plan_type,
        request_override,
    )
    .await
}

async fn persist_auth_token_with_plan(
    state: &AppState,
    tenant_id: Uuid,
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
        tenant_id,
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
    dash_auth::CurrentUser(current_user): dash_auth::CurrentUser,
    Query(query): Query<ListPageQuery>,
) -> AppResult<Json<ListPage<GptAccountResponse>>> {
    let page = query.normalize()?;
    let tenant_id = current_user.tenant_id.ok_or(AppError::Forbidden)?;
    let mut conn = state.db_conn().await?;
    let visible_group_ids = group_access::group_ids_with_permission(
        &mut conn,
        &current_user,
        GroupPermission::AccountView,
    )
    .await?;
    let override_group_ids = group_access::group_ids_with_permission(
        &mut conn,
        &current_user,
        GroupPermission::AccountOverrideView,
    )
    .await?
    .map(|ids| ids.into_iter().collect::<HashSet<_>>());
    drop(conn);
    let service = ProviderResourceService::<GptMaintenance>::new(&state);
    let snapshots = match visible_group_ids {
        None => {
            service
                .list_accounts(tenant_id, page.query_limit(), page.offset())
                .await?
        }
        Some(group_ids) => {
            service
                .list_accounts_in_groups(tenant_id, &group_ids, page.query_limit(), page.offset())
                .await?
        }
    };
    let items = snapshots
        .into_iter()
        .map(|snapshot| {
            let can_view_override = override_group_ids.as_ref().is_none_or(|ids| {
                snapshot
                    .account
                    .group_id
                    .is_some_and(|id| ids.contains(&id))
            });
            GptAccountResponse::from_snapshot(snapshot, can_view_override)
        })
        .collect::<AppResult<Vec<_>>>()?;

    Ok(Json(page.finish(items)))
}

async fn update_gpt_account_enabled(
    State(state): State<AppState>,
    dash_auth::AdminUser(owner): dash_auth::AdminUser,
    Path(id): Path<Uuid>,
    Json(payload): Json<UpdateGptAccountEnabledRequest>,
) -> AdminResult<Json<GptAccountResponse>> {
    let tenant_id = owner.tenant_id.ok_or(AppError::Forbidden)?;
    let snapshot = ProviderResourceService::<GptMaintenance>::new(&state)
        .update_account_enabled(tenant_id, id, payload.enabled)
        .await?;

    Ok(Json(GptAccountResponse::from_snapshot(snapshot, true)?))
}

async fn update_gpt_account_override(
    State(state): State<AppState>,
    dash_auth::CurrentUser(current_user): dash_auth::CurrentUser,
    Path(id): Path<Uuid>,
    Json(payload): Json<UpdateRequestOverrideRequest>,
) -> AppResult<Json<GptAccountResponse>> {
    payload.override_.validate()?;
    let tenant_id = current_user.tenant_id.ok_or(AppError::Forbidden)?;
    let service = ProviderResourceService::<GptMaintenance>::new(&state);
    let account = service
        .find_account(tenant_id, id)
        .await?
        .ok_or(AppError::Forbidden)?;
    let mut conn = state.db_conn().await?;
    group_access::require_permission(
        &mut conn,
        &current_user,
        account.group_id,
        GroupPermission::AccountOverrideUpdate,
    )
    .await?;
    drop(conn);
    let snapshot = if current_user.is_tenant_owner() {
        service
            .update_account_override(tenant_id, id, payload.override_)
            .await?
    } else {
        service
            .update_account_override_in_group(
                tenant_id,
                id,
                account.group_id.ok_or(AppError::Forbidden)?,
                payload.override_,
            )
            .await?
    };

    info!(
        actor_user_id = %current_user.id,
        gpt_account_id = %snapshot.account.id,
        "管理端更新 GPT 账号请求 override 成功，runtime 已同步"
    );
    Ok(Json(GptAccountResponse::from_snapshot(snapshot, true)?))
}

async fn update_gpt_account_group(
    State(state): State<AppState>,
    dash_auth::AdminUser(owner): dash_auth::AdminUser,
    Path(id): Path<Uuid>,
    Json(payload): Json<ProviderGroupRequest>,
) -> AdminResult<Json<GptAccountResponse>> {
    let tenant_id = owner.tenant_id.ok_or(AppError::Forbidden)?;
    let snapshot = ProviderResourceService::<GptMaintenance>::new(&state)
        .update_account_group(tenant_id, id, payload.group_id)
        .await?;
    info!(
        gpt_account_id = %snapshot.account.id,
        provider_group_id = ?snapshot.account.group_id,
        "管理端调整 GPT 账号分组成功，runtime 已同步"
    );
    Ok(Json(GptAccountResponse::from_snapshot(snapshot, true)?))
}

async fn refresh_gpt_account_quota(
    State(state): State<AppState>,
    dash_auth::CurrentUser(current_user): dash_auth::CurrentUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<quota::GptAccountQuotaResponse>> {
    let service = ProviderResourceService::<GptMaintenance>::new(&state);
    let tenant_id = current_user.tenant_id.ok_or(AppError::Forbidden)?;
    let account = service.find_account(tenant_id, id).await?.ok_or_else(|| {
        warn!(gpt_account_id = %id, "管理端刷新 GPT 账号额度失败，账号不存在");
        AppError::BadRequest {
            message: format!("GPT 账号不存在: {id}"),
        }
    })?;
    let mut conn = state.db_conn().await?;
    group_access::require_permission(
        &mut conn,
        &current_user,
        account.group_id,
        GroupPermission::AccountQuotaView,
    )
    .await?;
    drop(conn);

    // 只记录查询发起前仍生效的 quota 快照。查询期间若账号发生任意持久变化，后续 CAS
    // 会拒绝用旧的上游结果清理状态，防止覆盖新到达的额度耗尽回执。
    let limited_snapshot = account
        .quota_resets_at
        .filter(|quota_resets_at| *quota_resets_at > chrono::Utc::now())
        .map(|quota_resets_at| (quota_resets_at, account.updated_at));
    let mut quota = quota::fetch_account_quota(&state, &account).await?;
    let available_remaining_percent = quota.available_remaining_percent();

    if let (Some((expected_quota_resets_at, expected_updated_at)), Some(remaining_percent)) =
        (limited_snapshot, available_remaining_percent)
    {
        match service
            .clear_account_quota_limit_if_snapshot(
                tenant_id,
                account.id,
                expected_quota_resets_at,
                expected_updated_at,
            )
            .await?
        {
            Some(snapshot) => {
                quota.quota_limit_removed = true;
                info!(
                    gpt_account_id = %account.id,
                    minimum_remaining_percent = remaining_percent,
                    previous_quota_resets_at = %expected_quota_resets_at,
                    runtime_ready = snapshot.runtime.runtime_ready,
                    "GPT 账号额度已恢复，既有额度限制已清理并重新同步调度运行态"
                );
            }
            None => {
                warn!(
                    gpt_account_id = %account.id,
                    minimum_remaining_percent = remaining_percent,
                    expected_quota_resets_at = %expected_quota_resets_at,
                    expected_updated_at = %expected_updated_at,
                    "GPT 账号额度已恢复，但查询期间持久状态发生变化，已保留当前额度限制"
                );
            }
        }
    } else if limited_snapshot.is_some() {
        info!(
            gpt_account_id = %account.id,
            allowed = quota.primary.as_ref().and_then(|snapshot| snapshot.allowed),
            limit_reached = quota.primary.as_ref().and_then(|snapshot| snapshot.limit_reached),
            "GPT 账号仍无可用额度，保留现有额度限制"
        );
    }

    info!(
        actor_user_id = %current_user.id,
        gpt_account_id = %account.id,
        chatgpt_account_id = quota.chatgpt_account_id.as_deref().unwrap_or("<missing>"),
        snapshot_count = quota.snapshots.len(),
        quota_limit_removed = quota.quota_limit_removed,
        "管理端 GPT 账号额度已刷新"
    );

    Ok(Json(quota))
}

/// 查询指定 GPT OAuth 账号可用的人工额度重置记录。
async fn list_gpt_account_rate_limit_reset_credits(
    State(state): State<AppState>,
    dash_auth::CurrentUser(current_user): dash_auth::CurrentUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<rate_limit_reset::RateLimitResetCreditsResponse>> {
    let service = ProviderResourceService::<GptMaintenance>::new(&state);
    let tenant_id = current_user.tenant_id.ok_or(AppError::Forbidden)?;
    let account = service.find_account(tenant_id, id).await?.ok_or_else(|| {
        warn!(gpt_account_id = %id, "管理端查询 GPT 账号额度重置记录失败，账号不存在");
        AppError::BadRequest {
            message: format!("GPT 账号不存在: {id}"),
        }
    })?;
    let mut conn = state.db_conn().await?;
    group_access::require_permission(
        &mut conn,
        &current_user,
        account.group_id,
        GroupPermission::AccountResetView,
    )
    .await?;
    drop(conn);

    let response = rate_limit_reset::fetch_rate_limit_reset_credits(&state, &account).await?;
    info!(
        actor_user_id = %current_user.id,
        gpt_account_id = %account.id,
        available_count = response.available_count,
        credit_count = response.credits.len(),
        "管理端已查询 GPT 账号额度重置记录"
    );
    Ok(Json(response))
}

/// 应用一条由查询接口返回的额度重置记录。
async fn consume_gpt_account_rate_limit_reset_credit(
    State(state): State<AppState>,
    dash_auth::CurrentUser(current_user): dash_auth::CurrentUser,
    Path(id): Path<Uuid>,
    Json(payload): Json<rate_limit_reset::ConsumeRateLimitResetCreditRequest>,
) -> AppResult<Json<rate_limit_reset::ConsumeRateLimitResetCreditResponse>> {
    let service = ProviderResourceService::<GptMaintenance>::new(&state);
    let tenant_id = current_user.tenant_id.ok_or(AppError::Forbidden)?;
    let account = service.find_account(tenant_id, id).await?.ok_or_else(|| {
        warn!(gpt_account_id = %id, "管理端应用 GPT 账号额度重置记录失败，账号不存在");
        AppError::BadRequest {
            message: format!("GPT 账号不存在: {id}"),
        }
    })?;
    let mut conn = state.db_conn().await?;
    group_access::require_permission(
        &mut conn,
        &current_user,
        account.group_id,
        GroupPermission::AccountResetConsume,
    )
    .await?;
    drop(conn);

    // 上游 redeem_request_id 是本次后端操作的协议细节，不暴露给浏览器。UUID v7 同时便于
    // 按时间定位日志；当前管理接口不做透明自动重试，每次点击“应用”创建一次新操作。
    let idempotency_key = Uuid::now_v7();
    let response = rate_limit_reset::consume_rate_limit_reset_credit(
        &state,
        &account,
        idempotency_key,
        &payload.credit_id,
    )
    .await?;
    info!(
        actor_user_id = %current_user.id,
        gpt_account_id = %account.id,
        credit_id = %payload.credit_id,
        idempotency_key = %idempotency_key,
        outcome = ?response.code,
        windows_reset = response.windows_reset,
        "管理端已完成 GPT 账号额度重置操作"
    );
    Ok(Json(response))
}

async fn delete_gpt_account(
    State(state): State<AppState>,
    dash_auth::AdminUser(owner): dash_auth::AdminUser,
    Path(id): Path<Uuid>,
) -> AdminResult<Json<DeleteGptAccountResponse>> {
    let service = ProviderResourceService::<GptMaintenance>::new(&state);
    let tenant_id = owner.tenant_id.ok_or(AppError::Forbidden)?;
    let account_exists = service.find_account(tenant_id, id).await?.is_some();

    if !account_exists {
        warn!(gpt_account_id = %id, "管理端删除 GPT 账号失败，账号不存在");
        return Err(AppError::BadRequest {
            message: format!("GPT 账号不存在: {id}"),
        });
    }

    let deleted = service.delete_account(tenant_id, id).await?;

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
    fn from_snapshot(snapshot: AccountSnapshot, can_view_override: bool) -> AppResult<Self> {
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
            override_: can_view_override.then_some(request_override),
            runtime,
        })
    }
}
