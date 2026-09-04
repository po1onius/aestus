use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{get, put},
};
use serde::{Deserialize, Deserializer, Serialize};
use tracing::info;
use uuid::Uuid;

use crate::{
    api::dash::{
        auth,
        pagination::{ListPage, ListPageQuery},
    },
    err::{AdminResult, AppError},
    provider::{claude, gpt},
    request::concurrency,
    state::AppState,
    user::{
        self, PublicUser, User,
        group_access::{self, GroupGrantInput, UserGroupGrant},
    },
};

#[derive(Debug, Serialize)]
struct CurrentConcurrencyResponse {
    gpt: u32,
    claude: u32,
}

/// owner 用户列表专用 DTO。通过 flatten 保持原有用户字段形状，同时不向登录态等复用
/// `PublicUser` 的响应注入仅管理列表需要的 Redis 运行时状态。
#[derive(Debug, Serialize)]
struct UserListItemResponse {
    #[serde(flatten)]
    user: PublicUser,
    current_concurrency: CurrentConcurrencyResponse,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateUserQuotaRequest {
    quota: i64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateUserMaxConcurrencyRequest {
    /// `null` 表示不限制并发，非空值必须位于 1..=10000。
    max_concurrency: NullableMaxConcurrency,
}

/// 保留“字段必传、值允许为 null”的 API 语义；直接使用 `Option<i32>` 会让空对象也被
/// 反序列化成“不限”，从而可能因客户端漏传字段而误清除已有上限。
#[derive(Debug)]
struct NullableMaxConcurrency(Option<i32>);

impl<'de> Deserialize<'de> for NullableMaxConcurrency {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Option::<i32>::deserialize(deserializer).map(Self)
    }
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplaceUserGroupGrantsRequest {
    grants: Vec<UserGroupGrantRequest>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UserGroupGrantRequest {
    group_id: Uuid,
    #[serde(default)]
    permissions: Vec<String>,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list_users).post(create_user))
        .route("/{id}/quota", put(update_user_quota))
        .route("/{id}/max-concurrency", put(update_user_max_concurrency))
        .route("/{id}/status", put(update_user_status))
        .route(
            "/{id}/group-grants",
            get(list_user_group_grants).put(replace_user_group_grants),
        )
}

async fn list_user_group_grants(
    State(state): State<AppState>,
    auth::AdminUser(owner): auth::AdminUser,
    Path(id): Path<Uuid>,
) -> AdminResult<Json<Vec<UserGroupGrant>>> {
    let tenant_id = owner.tenant_id.ok_or(AppError::Forbidden)?;
    let mut conn = state.db_conn().await?;
    let grants = group_access::list_for_managed_user(&mut conn, tenant_id, id).await?;
    info!(
        admin_user_id = %owner.id,
        target_user_id = %id,
        tenant_id = %tenant_id,
        group_grant_count = grants.len(),
        "租户 owner 已读取普通用户的 Provider 分组授权"
    );
    Ok(Json(grants))
}

async fn replace_user_group_grants(
    State(state): State<AppState>,
    auth::AdminUser(owner): auth::AdminUser,
    Path(id): Path<Uuid>,
    Json(payload): Json<ReplaceUserGroupGrantsRequest>,
) -> AdminResult<Json<Vec<UserGroupGrant>>> {
    let tenant_id = owner.tenant_id.ok_or(AppError::Forbidden)?;
    let inputs = payload
        .grants
        .into_iter()
        .map(|grant| GroupGrantInput {
            group_id: grant.group_id,
            permissions: grant.permissions,
        })
        .collect();
    let mut conn = state.db_conn().await?;
    let grants =
        group_access::replace_for_managed_user(&mut conn, tenant_id, id, owner.id, inputs).await?;
    Ok(Json(grants))
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
) -> AdminResult<Json<ListPage<UserListItemResponse>>> {
    let page = query.normalize()?;
    let mut conn = state.db_conn().await?;
    let tenant_id = owner.tenant_id.ok_or(AppError::Forbidden)?;
    let users =
        user::list_by_tenant(&mut conn, tenant_id, page.query_limit(), page.offset()).await?;
    drop(conn);

    // 先完成分页截断，避免为仅用于判断 next_offset 的额外一行查询 Redis；随后按原 Vec
    // 顺序组装 DTO，保持 PostgreSQL 用户列表的稳定排序。
    let user_page = page.finish(users);
    let user_ids = user_page
        .items
        .iter()
        .map(|user| user.id)
        .collect::<Vec<_>>();
    let providers = [gpt::model::PROVIDER, claude::model::PROVIDER];
    let current =
        concurrency::active_counts_for_users(&state, tenant_id, &user_ids, &providers).await?;
    let items = user_page
        .items
        .into_iter()
        .map(|user| {
            let user_id = user.id;
            UserListItemResponse {
                user: PublicUser::from(user),
                current_concurrency: CurrentConcurrencyResponse {
                    gpt: current.count(user_id, gpt::model::PROVIDER),
                    claude: current.count(user_id, claude::model::PROVIDER),
                },
            }
        })
        .collect();

    info!(
        admin_user_id = %owner.id,
        admin_username = %owner.username,
        tenant_id = %tenant_id,
        user_count = user_ids.len(),
        "租户 owner 用户列表已附加 provider 实时并发"
    );

    Ok(Json(ListPage {
        items,
        offset: user_page.offset,
        limit: user_page.limit,
        next_offset: user_page.next_offset,
    }))
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

async fn update_user_max_concurrency(
    State(state): State<AppState>,
    auth::AdminUser(owner): auth::AdminUser,
    Path(id): Path<Uuid>,
    Json(payload): Json<UpdateUserMaxConcurrencyRequest>,
) -> AdminResult<Json<PublicUser>> {
    let mut conn = state.db_conn().await?;
    let tenant_id = owner.tenant_id.ok_or(AppError::Forbidden)?;
    let requested_max_concurrency = payload.max_concurrency.0;
    let (user, previous_max_concurrency) = User::update_max_concurrency_for_tenant(
        &mut conn,
        tenant_id,
        id,
        requested_max_concurrency,
    )
    .await?;

    info!(
        admin_user_id = %owner.id,
        admin_username = %owner.username,
        tenant_id = %tenant_id,
        target_user_id = %user.id,
        target_username = %user.username,
        previous_max_concurrency = ?previous_max_concurrency,
        max_concurrency = ?user.max_concurrency,
        "租户 owner 已通过 Dashboard 更新用户最大并发数"
    );

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
