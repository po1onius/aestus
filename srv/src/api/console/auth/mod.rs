use axum::{
    Extension, Json, Router,
    extract::State,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::{
    api::{
        console::{audit::AuditContext, error::ConsoleResult},
        rate_limit::PublicRateLimitRuntime,
    },
    config::PublicRateLimitConfig,
    err::{AppError, AppResult, ConsoleError},
    state::AppState,
    tenant::{self, Tenant},
    user::{self, PublicUser, User},
};

mod extractor;

pub(crate) use extractor::{AdminUser, CurrentUser, PlatformAdminUser};

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
    id: String,
    #[serde(flatten)]
    limits: tenant::TenantLimits,
}

impl From<Tenant> for PublicTenant {
    fn from(tenant: Tenant) -> Self {
        Self {
            limits: tenant.limits(),
            id: tenant.id,
        }
    }
}

pub(super) fn router(
    limits: &PublicRateLimitConfig,
    runtime: &mut PublicRateLimitRuntime,
) -> Router<AppState> {
    Router::new()
        .route(
            "/register/email-code",
            runtime.limit(
                post(send_register_email_code),
                "/api/console/auth/register/email-code",
                limits.email_code,
            ),
        )
        .route(
            "/register",
            runtime.limit(
                post(register),
                "/api/console/auth/register",
                limits.register,
            ),
        )
        .route(
            "/login",
            runtime.limit(post(login), "/api/console/auth/login", limits.login),
        )
        .route("/me", get(me))
}

async fn send_register_email_code(
    State(state): State<AppState>,
    Json(payload): Json<SendRegisterEmailCodeRequest>,
) -> ConsoleResult<Json<SendRegisterEmailCodeResponse<'static>>> {
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
    Extension(audit): Extension<AuditContext>,
    Json(payload): Json<RegisterRequest>,
) -> ConsoleResult<Json<AuthResponse>> {
    let username = user::normalize_username(&payload.username)?;
    let email = user::normalize_email(&payload.email)?;
    let tenant_code = tenant::normalize_code(payload.tenant_code)?;
    // 所有纯本地校验必须在消费一次性验证码之前完成，避免密码边界错误浪费有效验证码。
    user::validate_registration_password(&payload.password)?;
    // 租户码是平台管理员主动分发的明文凭证，先确认仍存在且租户可用，避免无效或已撤销
    // 的租户码消费一次性邮箱验证码；后续注册事务会再次加锁校验并原子裁决 owner 身份。
    let mut conn = state.db_conn().await?;
    let registration_tenant = tenant::find_enabled_by_code(&mut conn, &tenant_code).await?;
    if registration_tenant.is_none() {
        warn!(email, "用户使用无效、已撤销或已停用租户的租户码注册");
        return Err(AppError::BadRequest {
            message: "租户码无效或已撤销".to_owned(),
        }
        .into());
    }
    // 预检查避免已满租户消耗验证码；最终准入仍由创建事务加锁裁决。
    if let Some(tenant) = registration_tenant {
        tenant::require_user_capacity(&mut conn, &tenant).await?;
    }
    drop(conn);
    // 唯一性检查必须晚于验证码校验，避免公开注册接口被用来枚举用户名或邮箱；并发竞态
    // 最终仍由 PostgreSQL 唯一索引原子裁决。
    user::verify_register_email_code(&state, &email, &payload.email_code).await?;
    let mut conn = state.db_conn().await?;
    let user =
        user::register_with_tenant_code(&mut conn, tenant_code, username, email, payload.password)
            .await?;
    audit.record_user(&user);
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
    Extension(audit): Extension<AuditContext>,
    Json(payload): Json<LoginRequest>,
) -> ConsoleResult<Json<AuthResponse>> {
    let mut conn = state.db_conn().await?;
    let Some(user) = user::find_by_login_identifier(&mut conn, &payload.identifier).await? else {
        // 不存在或格式无效的登录标识也执行一次 bcrypt，降低通过响应耗时枚举账号的可行性。
        user::burn_dummy_password_verification().await?;
        warn!(
            identifier_has_at = payload.identifier.contains('@'),
            "不存在或格式无效的登录标识尝试登录"
        );
        return Err(AppError::Console(ConsoleError::InvalidDashboardToken).into());
    };
    if !user::verify_password(payload.password, user.password_hash.clone()).await? {
        warn!(user_id = %user.id, username = %user.username, "用户登录密码错误");
        return Err(AppError::Console(ConsoleError::InvalidDashboardToken).into());
    }
    audit.record_user(&user);
    if !user.enabled {
        warn!(user_id = %user.id, username = %user.username, email = %user.email, "禁用用户尝试登录");
        return Err(AppError::Console(ConsoleError::InvalidDashboardToken).into());
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
) -> ConsoleResult<Json<MeResponse>> {
    let mut conn = state.db_conn().await?;
    let tenant = load_public_tenant(&mut conn, &user).await?;
    Ok(Json(MeResponse {
        user: user.into(),
        tenant,
        service_timezone: state.config().service_timezone.name().to_owned(),
        request_log_retention_days: state.config().request_log_retention_days.get(),
    }))
}

async fn load_public_tenant(
    conn: &mut diesel_async::AsyncPgConnection,
    user: &User,
) -> AppResult<Option<PublicTenant>> {
    let Some(tenant_id) = user.tenant_id.as_deref() else {
        return Ok(None);
    };
    tenant::require_enabled(conn, tenant_id)
        .await
        .map(PublicTenant::from)
        .map(Some)
}
