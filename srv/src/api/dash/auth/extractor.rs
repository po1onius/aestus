use axum::{
    extract::FromRequestParts,
    http::{HeaderMap, header::AUTHORIZATION, request::Parts},
};
use tracing::warn;

use crate::{
    err::{AppError, AppResult},
    state::AppState,
    tenant,
    user::{self, User},
};

const BEARER_PREFIX: &str = "Bearer ";

/// 已通过 Dashboard 用户鉴权的请求主体。
///
/// 鉴权作为 extractor 在业务 handler 之前完成；其拒绝响应始终使用公共脱敏错误，避免
/// 数据库故障时把内部诊断暴露给尚未证明身份的请求。
pub(crate) struct CurrentUser(pub User);

/// 已通过租户 owner 角色校验的请求主体。
pub(crate) struct AdminUser(pub User);

/// 平台管理员只负责平台级租户生命周期，不隐式获得某个租户的资源写入作用域。
pub(crate) struct PlatformAdminUser(pub User);

impl FromRequestParts<AppState> for CurrentUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        require_user(state, &parts.headers).await.map(Self)
    }
}

impl FromRequestParts<AppState> for AdminUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        require_admin(state, &parts.headers).await.map(Self)
    }
}

impl FromRequestParts<AppState> for PlatformAdminUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        require_platform_admin(state, &parts.headers)
            .await
            .map(Self)
    }
}

pub(crate) async fn require_user(state: &AppState, headers: &HeaderMap) -> AppResult<User> {
    let token = extract_bearer_token(headers)?;
    let claims = user::decode_jwt(state, token)?;
    let mut conn = state.db_conn().await?;
    let user = user::find_by_id(&mut conn, claims.sub)
        .await?
        .ok_or(AppError::InvalidDashboardToken)?;

    if !user.enabled {
        warn!(user_id = %user.id, username = %user.username, email = %user.email, "Dashboard JWT 对应用户已禁用");
        return Err(AppError::Forbidden);
    }

    if let Some(tenant_id) = user.tenant_id.as_deref() {
        tenant::require_enabled(&mut conn, tenant_id).await?;
    }

    Ok(user)
}

pub(crate) async fn require_admin(state: &AppState, headers: &HeaderMap) -> AppResult<User> {
    let user = require_user(state, headers).await?;
    if !user.is_tenant_owner() {
        warn!(user_id = %user.id, role = %user.role, tenant_id = ?user.tenant_id, "非租户 owner 用户访问租户管理接口");
        return Err(AppError::Forbidden);
    }
    Ok(user)
}

pub(crate) async fn require_platform_admin(
    state: &AppState,
    headers: &HeaderMap,
) -> AppResult<User> {
    let user = require_user(state, headers).await?;
    if !user.is_platform_admin() {
        warn!(user_id = %user.id, role = %user.role, tenant_id = ?user.tenant_id, "非平台管理员访问平台管理接口");
        return Err(AppError::Forbidden);
    }
    Ok(user)
}

fn extract_bearer_token(headers: &HeaderMap) -> AppResult<&str> {
    let raw_value = headers
        .get(AUTHORIZATION)
        .ok_or(AppError::MissingDashboardToken)?
        .to_str()
        .map_err(|_| AppError::MissingDashboardToken)?;

    raw_value
        .strip_prefix(BEARER_PREFIX)
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .ok_or(AppError::MissingDashboardToken)
}
