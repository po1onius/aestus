use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, header::CACHE_CONTROL},
    routing::{get, post, put},
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    api::dash::auth,
    err::AdminResult,
    state::AppState,
    tenant::{self, Tenant, TenantSummary},
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateTenantRequest {
    name: String,
    /// 缺失、null 或空字符串时只创建租户；非空时同时创建租户 owner。
    password: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateTenantStatusRequest {
    enabled: bool,
}

#[derive(Debug, Serialize)]
struct RevokeTenantCodeResponse {
    tenant_id: Uuid,
}

type PrivateJson<T> = (HeaderMap, Json<T>);

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list_tenants).post(create_tenant))
        .route("/{id}/status", put(update_tenant_status))
        .route(
            "/{id}/code",
            post(regenerate_tenant_code).delete(revoke_tenant_code),
        )
}

async fn create_tenant(
    State(state): State<AppState>,
    auth::PlatformAdminUser(admin): auth::PlatformAdminUser,
    Json(payload): Json<CreateTenantRequest>,
) -> AdminResult<PrivateJson<TenantSummary>> {
    let mut conn = state.db_conn().await?;
    Ok(private_json(
        tenant::create(&mut conn, payload.name, payload.password, admin.id).await?,
    ))
}

async fn list_tenants(
    State(state): State<AppState>,
    _admin: auth::PlatformAdminUser,
) -> AdminResult<PrivateJson<Vec<TenantSummary>>> {
    let mut conn = state.db_conn().await?;
    Ok(private_json(tenant::list(&mut conn).await?))
}

async fn update_tenant_status(
    State(state): State<AppState>,
    auth::PlatformAdminUser(admin): auth::PlatformAdminUser,
    Path(id): Path<Uuid>,
    Json(payload): Json<UpdateTenantStatusRequest>,
) -> AdminResult<Json<Tenant>> {
    let mut conn = state.db_conn().await?;
    Ok(Json(
        tenant::set_enabled(&mut conn, id, payload.enabled, admin.id).await?,
    ))
}

async fn regenerate_tenant_code(
    State(state): State<AppState>,
    auth::PlatformAdminUser(admin): auth::PlatformAdminUser,
    Path(id): Path<Uuid>,
) -> AdminResult<PrivateJson<TenantSummary>> {
    let mut conn = state.db_conn().await?;
    Ok(private_json(
        tenant::regenerate_code(&mut conn, id, admin.id).await?,
    ))
}

async fn revoke_tenant_code(
    State(state): State<AppState>,
    auth::PlatformAdminUser(admin): auth::PlatformAdminUser,
    Path(id): Path<Uuid>,
) -> AdminResult<Json<RevokeTenantCodeResponse>> {
    let mut conn = state.db_conn().await?;
    tenant::revoke_code(&mut conn, id, admin.id).await?;
    Ok(Json(RevokeTenantCodeResponse { tenant_id: id }))
}

fn private_json<T>(value: T) -> PrivateJson<T> {
    let mut headers = HeaderMap::new();
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    (headers, Json(value))
}
