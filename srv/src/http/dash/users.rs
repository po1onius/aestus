use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{get, put},
};
use serde::Deserialize;
use tracing::info;
use uuid::Uuid;

use crate::{
    err::{AdminResult, AppError},
    http::dash::{
        auth,
        pagination::{ListPage, ListPageQuery},
    },
    state::AppState,
    user::{self, PublicUser},
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateUserQuotaRequest {
    quota: i64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateUserStatusRequest {
    enabled: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateUserRequest {
    username: String,
    /// 缺失、null 或纯空白邮箱都表示使用服务端生成的 `用户名@aes.tus` 默认地址。
    email: Option<String>,
    password: String,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list_users).post(create_user))
        .route("/{id}/quota", put(update_user_quota))
        .route("/{id}/status", put(update_user_status))
}

async fn create_user(
    State(state): State<AppState>,
    auth::AdminUser(current_admin): auth::AdminUser,
    Json(payload): Json<CreateUserRequest>,
) -> AdminResult<Json<PublicUser>> {
    let mut conn = state.db_conn().await?;
    let tenant_id = current_admin.tenant_id.ok_or(AppError::Forbidden)?;
    let user = user::create_owner_managed_user(
        &mut conn,
        tenant_id,
        payload.username,
        payload.email,
        payload.password,
    )
    .await?;

    // 记录操作者与目标用户的审计信息，但绝不把密码或密码哈希写入日志。
    info!(
        admin_user_id = %current_admin.id,
        admin_username = %current_admin.username,
        created_user_id = %user.id,
        created_username = %user.username,
        created_email = %user.email,
        tenant_id = %tenant_id,
        "租户 owner 已通过 Dashboard 创建用户"
    );

    Ok(Json(user.into()))
}

async fn list_users(
    State(state): State<AppState>,
    auth::AdminUser(owner): auth::AdminUser,
    Query(query): Query<ListPageQuery>,
) -> AdminResult<Json<ListPage<PublicUser>>> {
    let page = query.normalize()?;
    let mut conn = state.db_conn().await?;
    let tenant_id = owner.tenant_id.ok_or(AppError::Forbidden)?;
    let items = user::list_by_tenant(&mut conn, tenant_id, page.query_limit(), page.offset())
        .await?
        .into_iter()
        .map(PublicUser::from)
        .collect();

    Ok(Json(page.finish(items)))
}

async fn update_user_quota(
    State(state): State<AppState>,
    auth::AdminUser(owner): auth::AdminUser,
    Path(id): Path<Uuid>,
    Json(payload): Json<UpdateUserQuotaRequest>,
) -> AdminResult<Json<PublicUser>> {
    let mut conn = state.db_conn().await?;
    let tenant_id = owner.tenant_id.ok_or(AppError::Forbidden)?;
    let user = user::update_quota_for_tenant(&mut conn, tenant_id, id, payload.quota).await?;

    Ok(Json(user.into()))
}

async fn update_user_status(
    State(state): State<AppState>,
    auth::AdminUser(current_user): auth::AdminUser,
    Path(id): Path<Uuid>,
    Json(payload): Json<UpdateUserStatusRequest>,
) -> AdminResult<Json<PublicUser>> {
    let mut conn = state.db_conn().await?;
    let tenant_id = current_user.tenant_id.ok_or(AppError::Forbidden)?;
    let user = user::update_status(&mut conn, tenant_id, id, payload.enabled).await?;

    Ok(Json(user.into()))
}
