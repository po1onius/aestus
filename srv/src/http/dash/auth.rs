use axum::{
    Json, Router,
    extract::{FromRequestParts, State},
    http::request::Parts,
    http::{HeaderMap, header::AUTHORIZATION},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::{
    err::{AppError, AppResult},
    model::User,
    state::AppState,
    tenant::{self, Tenant},
    user::{self, PublicUser},
};

const BEARER_PREFIX: &str = "Bearer ";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SendRegisterEmailCodeRequest {
    email: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RegisterRequest {
    tenant_code: String,
    username: String,
    email: String,
    password: String,
    email_code: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LoginRequest {
    identifier: String,
    password: String,
}

#[derive(Debug, Serialize)]
struct AuthResponse {
    token: String,
    user: PublicUser,
    tenant: Option<PublicTenant>,
    service_timezone: String,
    request_log_retention_days: u32,
}

#[derive(Debug, Serialize)]
struct MeResponse {
    user: PublicUser,
    tenant: Option<PublicTenant>,
    service_timezone: String,
    request_log_retention_days: u32,
}

#[derive(Debug, Serialize)]
struct SendRegisterEmailCodeResponse<'a> {
    status: &'a str,
}

#[derive(Debug, Serialize)]
struct PublicTenant {
    id: uuid::Uuid,
    name: String,
}

impl From<Tenant> for PublicTenant {
    fn from(tenant: Tenant) -> Self {
        Self {
            id: tenant.id,
            name: tenant.name,
        }
    }
}

/// 已通过 Dashboard 用户鉴权的请求主体。
///
/// 鉴权作为 extractor 在业务 handler 之前完成；其拒绝响应始终使用公共脱敏错误，避免
/// 数据库故障时把内部诊断暴露给尚未证明身份的请求。
pub(crate) struct CurrentUser(pub User);

/// 已通过租户 owner 角色校验的请求主体。
pub(crate) struct AdminUser(pub User);

/// 平台管理员只负责平台级租户生命周期，不隐式获得某个租户的资源写入作用域。
pub(crate) struct PlatformAdminUser(pub User);

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/register/email-code", post(send_register_email_code))
        .route("/register", post(register))
        .route("/login", post(login))
        .route("/me", get(me))
}

async fn send_register_email_code(
    State(state): State<AppState>,
    Json(payload): Json<SendRegisterEmailCodeRequest>,
) -> AppResult<Json<SendRegisterEmailCodeResponse<'static>>> {
    let email = user::normalize_email(&payload.email)?;
    let mut conn = state.db_conn().await?;
    if user::find_by_email(&mut conn, &email).await?.is_some() {
        // 未注册和已注册邮箱统一返回成功，避免公开接口成为账号枚举器。已注册邮箱不重复
        // 发送邮件，详细命中情况只进入服务端日志。
        info!(email, "注册验证码请求命中已注册邮箱，返回通用成功响应");
        return Ok(Json(SendRegisterEmailCodeResponse { status: "ok" }));
    }
    drop(conn);

    user::send_register_email_code(&state, email).await?;
    Ok(Json(SendRegisterEmailCodeResponse { status: "ok" }))
}

async fn register(
    State(state): State<AppState>,
    Json(payload): Json<RegisterRequest>,
) -> AppResult<Json<AuthResponse>> {
    let username = user::normalize_username(&payload.username)?;
    let email = user::normalize_email(&payload.email)?;
    let tenant_code = tenant::normalize_code(payload.tenant_code)?;
    // 所有纯本地校验必须在消费一次性验证码之前完成，避免密码边界错误浪费有效验证码。
    user::validate_registration_password(&payload.password)?;
    // 租户码是平台管理员主动分发的明文凭证，先确认仍存在且租户可用，避免无效或已撤销
    // 的租户码消费一次性邮箱验证码；后续注册事务会再次加锁校验并原子裁决 owner 身份。
    let mut conn = state.db_conn().await?;
    if tenant::find_enabled_by_code(&mut conn, &tenant_code)
        .await?
        .is_none()
    {
        warn!(email, "用户使用无效、已撤销或已停用租户的租户码注册");
        return Err(AppError::BadRequest {
            message: "租户码无效或已撤销".to_owned(),
        });
    }
    drop(conn);
    // 唯一性检查必须晚于验证码校验，避免公开注册接口被用来枚举用户名或邮箱；并发竞态
    // 最终仍由 PostgreSQL 唯一索引原子裁决。
    user::verify_register_email_code(&state, &email, &payload.email_code).await?;
    let mut conn = state.db_conn().await?;
    let user =
        user::register_with_tenant_code(&mut conn, tenant_code, username, email, payload.password)
            .await?;
    let tenant = load_public_tenant(&mut conn, &user).await?;
    let token = user::issue_jwt(&state, &user)?;

    info!(user_id = %user.id, username = %user.username, email = %user.email, "用户邮箱注册完成并签发 JWT");

    Ok(Json(AuthResponse {
        token,
        user: user.into(),
        tenant,
        service_timezone: state.config().service_timezone.name().to_owned(),
        request_log_retention_days: state.config().request_log_retention_days.get(),
    }))
}

async fn login(
    State(state): State<AppState>,
    Json(payload): Json<LoginRequest>,
) -> AppResult<Json<AuthResponse>> {
    let mut conn = state.db_conn().await?;
    let Some(user) = user::find_by_login_identifier(&mut conn, &payload.identifier).await? else {
        // 不存在或格式无效的登录标识也执行一次 bcrypt，降低通过响应耗时枚举账号的可行性。
        user::burn_dummy_password_verification().await?;
        warn!(
            identifier_has_at = payload.identifier.contains('@'),
            "不存在或格式无效的登录标识尝试登录"
        );
        return Err(AppError::InvalidDashboardToken);
    };
    if !user::verify_password(payload.password, user.password_hash.clone()).await? {
        warn!(user_id = %user.id, username = %user.username, "用户登录密码错误");
        return Err(AppError::InvalidDashboardToken);
    }
    if !user.enabled {
        warn!(user_id = %user.id, username = %user.username, email = %user.email, "禁用用户尝试登录");
        return Err(AppError::InvalidDashboardToken);
    }

    let tenant = load_public_tenant(&mut conn, &user).await?;
    let token = user::issue_jwt(&state, &user)?;
    info!(user_id = %user.id, username = %user.username, email = %user.email, "用户登录成功并签发 JWT");

    Ok(Json(AuthResponse {
        token,
        user: user.into(),
        tenant,
        service_timezone: state.config().service_timezone.name().to_owned(),
        request_log_retention_days: state.config().request_log_retention_days.get(),
    }))
}

async fn me(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
) -> AppResult<Json<MeResponse>> {
    let mut conn = state.db_conn().await?;
    let tenant = load_public_tenant(&mut conn, &user).await?;
    Ok(Json(MeResponse {
        user: user.into(),
        tenant,
        service_timezone: state.config().service_timezone.name().to_owned(),
        request_log_retention_days: state.config().request_log_retention_days.get(),
    }))
}

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

    if let Some(tenant_id) = user.tenant_id {
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

async fn load_public_tenant(
    conn: &mut diesel_async::AsyncPgConnection,
    user: &User,
) -> AppResult<Option<PublicTenant>> {
    let Some(tenant_id) = user.tenant_id else {
        return Ok(None);
    };
    tenant::require_enabled(conn, tenant_id)
        .await
        .map(PublicTenant::from)
        .map(Some)
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
