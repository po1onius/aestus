use std::collections::HashMap;

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, header::CACHE_CONTROL},
    routing::{get, post, put},
};
use serde::{Deserialize, Serialize};
use tracing::info;
use uuid::Uuid;

use crate::{
    api::dash::{
        auth,
        pagination::{ListPage, ListPageQuery},
    },
    err::{AdminResult, AppError, AppResult},
    provider::{
        claude::model::ClaudeAccountSpecific,
        credential::{ProviderAccount, ProviderApiKey},
        gpt::model::GptAccountSpecific,
        resource::UpstreamResourceKind,
        scheduler, sql,
    },
    state::AppState,
    tenant::{self, Tenant, TenantSummary},
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateTenantRequest {
    id: String,
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
    tenant_id: String,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TenantResourceKind {
    Account,
    OfficialApiKey,
}

impl TenantResourceKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Account => "account",
            Self::OfficialApiKey => "official_api_key",
        }
    }
}

#[derive(Debug, Deserialize)]
struct ListTenantResourcesQuery {
    kind: TenantResourceKind,
    limit: Option<i64>,
    offset: Option<i64>,
}

/// 平台视角只返回资源审计所需字段，不复用租户 owner DTO，避免意外暴露凭证和可写配置。
#[derive(Debug, Serialize)]
#[serde(tag = "resource_type", rename_all = "snake_case")]
enum TenantResourceResponse {
    Account {
        id: Uuid,
        provider: String,
        email: Option<String>,
        plan: String,
        enabled: bool,
        status: String,
        status_reason: Option<String>,
        inflight_count: i64,
    },
    OfficialApiKey {
        id: Uuid,
        provider: String,
        base_url: String,
        enabled: bool,
        status: String,
        error: Option<String>,
        inflight_count: i64,
    },
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
        .route("/{id}/resources", get(list_tenant_resources))
}

async fn create_tenant(
    State(state): State<AppState>,
    auth::PlatformAdminUser(admin): auth::PlatformAdminUser,
    Json(payload): Json<CreateTenantRequest>,
) -> AdminResult<PrivateJson<TenantSummary>> {
    let mut conn = state.db_conn().await?;
    Ok(private_json(
        tenant::create(&mut conn, payload.id, payload.password, admin.id).await?,
    ))
}

async fn list_tenants(
    State(state): State<AppState>,
    _admin: auth::PlatformAdminUser,
) -> AdminResult<PrivateJson<Vec<TenantSummary>>> {
    let mut conn = state.db_conn().await?;
    Ok(private_json(tenant::list(&mut conn).await?))
}

async fn list_tenant_resources(
    State(state): State<AppState>,
    auth::PlatformAdminUser(admin): auth::PlatformAdminUser,
    Path(tenant_id): Path<String>,
    Query(query): Query<ListTenantResourcesQuery>,
) -> AdminResult<PrivateJson<ListPage<TenantResourceResponse>>> {
    let page = ListPageQuery::new(query.limit, query.offset).normalize()?;
    let mut conn = state.db_conn().await?;
    if tenant::find_by_id(&mut conn, &tenant_id).await?.is_none() {
        return Err(AppError::BadRequest {
            message: format!("租户不存在: {tenant_id}"),
        });
    }

    let items = match query.kind {
        TenantResourceKind::Account => {
            let accounts = sql::account::list_page_by_tenant(
                &mut conn,
                tenant_id.clone(),
                page.query_limit(),
                page.offset(),
            )
            .await?;
            drop(conn);
            account_resource_responses(&state, accounts).await?
        }
        TenantResourceKind::OfficialApiKey => {
            let api_keys = sql::api_key::list_page_by_tenant(
                &mut conn,
                tenant_id.clone(),
                page.query_limit(),
                page.offset(),
            )
            .await?;
            drop(conn);
            api_key_resource_responses(&state, api_keys).await?
        }
    };
    let response = page.finish(items);
    info!(
        platform_admin_id = %admin.id,
        tenant_id = %tenant_id,
        resource_type = query.kind.as_str(),
        offset = response.offset,
        limit = response.limit,
        returned_count = response.items.len(),
        next_offset = ?response.next_offset,
        "平台管理员已查看租户上游资源"
    );
    Ok(private_json(response))
}

async fn account_resource_responses(
    state: &AppState,
    accounts: Vec<ProviderAccount>,
) -> AppResult<Vec<TenantResourceResponse>> {
    let inflight_counts = load_inflight_counts(
        state,
        UpstreamResourceKind::Account,
        accounts
            .iter()
            .map(|account| (account.provider.as_str(), account.id)),
    )
    .await?;

    accounts
        .into_iter()
        .map(|account| {
            let (email, plan) = account_identity(&account)?;
            Ok(TenantResourceResponse::Account {
                id: account.id,
                provider: account.provider,
                email,
                plan,
                enabled: account.enabled,
                status: account.status,
                status_reason: account.status_reason,
                inflight_count: inflight_counts.get(&account.id).copied().unwrap_or(0),
            })
        })
        .collect()
}

async fn api_key_resource_responses(
    state: &AppState,
    api_keys: Vec<ProviderApiKey>,
) -> AppResult<Vec<TenantResourceResponse>> {
    let inflight_counts = load_inflight_counts(
        state,
        UpstreamResourceKind::ApiKey,
        api_keys
            .iter()
            .map(|api_key| (api_key.provider.as_str(), api_key.id)),
    )
    .await?;

    Ok(api_keys
        .into_iter()
        .map(|api_key| TenantResourceResponse::OfficialApiKey {
            id: api_key.id,
            provider: api_key.provider,
            base_url: api_key.base_url,
            enabled: api_key.enabled,
            status: if api_key.next_probe_at.is_some() {
                "unavailable".to_owned()
            } else {
                "valid".to_owned()
            },
            error: api_key.error,
            inflight_count: inflight_counts.get(&api_key.id).copied().unwrap_or(0),
        })
        .collect())
}

fn account_identity(account: &ProviderAccount) -> AppResult<(Option<String>, String)> {
    match account.provider.as_str() {
        crate::provider::gpt::model::PROVIDER => {
            let specific = account.parse_specific::<GptAccountSpecific>()?;
            Ok((specific.email, specific.plan_type))
        }
        crate::provider::claude::model::PROVIDER => {
            let specific = account.parse_specific::<ClaudeAccountSpecific>()?;
            Ok((
                specific.email_address,
                specific
                    .subscription_type
                    .map(|subscription| subscription.as_str().to_owned())
                    .unwrap_or_else(|| "unknown".to_owned()),
            ))
        }
        _ => Ok((None, "unknown".to_owned())),
    }
}

async fn load_inflight_counts<'a>(
    state: &AppState,
    kind: UpstreamResourceKind,
    resources: impl Iterator<Item = (&'a str, Uuid)>,
) -> AppResult<HashMap<Uuid, i64>> {
    let mut ids_by_provider = HashMap::<String, Vec<Uuid>>::new();
    for (provider, id) in resources {
        ids_by_provider
            .entry(provider.to_owned())
            .or_default()
            .push(id);
    }

    let mut inflight_counts = HashMap::new();
    for (provider, ids) in ids_by_provider {
        let views = scheduler::load_kind_views(state, &provider, kind, &ids).await?;
        inflight_counts.extend(
            views
                .into_iter()
                .map(|(id, view)| (id, view.inflight_count)),
        );
    }
    Ok(inflight_counts)
}

async fn update_tenant_status(
    State(state): State<AppState>,
    auth::PlatformAdminUser(admin): auth::PlatformAdminUser,
    Path(id): Path<String>,
    Json(payload): Json<UpdateTenantStatusRequest>,
) -> AdminResult<Json<Tenant>> {
    let mut conn = state.db_conn().await?;
    Ok(Json(
        tenant::set_enabled(&mut conn, &id, payload.enabled, admin.id).await?,
    ))
}

async fn regenerate_tenant_code(
    State(state): State<AppState>,
    auth::PlatformAdminUser(admin): auth::PlatformAdminUser,
    Path(id): Path<String>,
) -> AdminResult<PrivateJson<TenantSummary>> {
    let mut conn = state.db_conn().await?;
    Ok(private_json(
        tenant::regenerate_code(&mut conn, &id, admin.id).await?,
    ))
}

async fn revoke_tenant_code(
    State(state): State<AppState>,
    auth::PlatformAdminUser(admin): auth::PlatformAdminUser,
    Path(id): Path<String>,
) -> AdminResult<Json<RevokeTenantCodeResponse>> {
    let mut conn = state.db_conn().await?;
    tenant::revoke_code(&mut conn, &id, admin.id).await?;
    Ok(Json(RevokeTenantCodeResponse { tenant_id: id }))
}

fn private_json<T>(value: T) -> PrivateJson<T> {
    let mut headers = HeaderMap::new();
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    (headers, Json(value))
}
