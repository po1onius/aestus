mod account;
pub(super) mod audit;
pub(crate) mod auth;
mod gateway_api_keys;
mod pagination;
mod plugins;
mod provider_groups;
mod provider_upstream_api_keys;
mod statistics;
mod tenants;
mod users;

use axum::{Json, Router, http::StatusCode, response::IntoResponse, routing::get};
use serde::Serialize;

use crate::{
    api::rate_limit::PublicRateLimitRuntime,
    config::PublicRateLimitConfig,
    provider::{claude::maintenance::ClaudeMaintenance, gpt::maintenance::GptMaintenance},
    state::AppState,
};

#[derive(Debug, Serialize)]
struct ConsoleStatusResponse<'a> {
    status: &'a str,
    note: &'a str,
}

/// 控制台 API 路由，统一挂载于 `/api/console`。
///
/// 按业务资源组织 URL；角色权限和租户数据范围由各接口的鉴权逻辑确定。
pub(super) fn router(
    limits: &PublicRateLimitConfig,
    runtime: &mut PublicRateLimitRuntime,
) -> Router<AppState> {
    tracing::info!(api_prefix = "/api/console", "注册控制台 API 路由");
    Router::new()
        .route(
            "/status",
            runtime.limit(get(status), "/api/console/status", limits.status),
        )
        .nest("/auth", auth::router(limits, runtime))
        .nest("/gateway-api-keys", gateway_api_keys::router())
        .nest("/plugins", plugins::router())
        .nest("/plugin-suites", plugins::suites_router())
        .nest("/provider-groups", provider_groups::router())
        .nest("/providers/claude/accounts", account::claude::router())
        .nest(
            "/providers/claude/upstream-api-keys",
            provider_upstream_api_keys::router::<ClaudeMaintenance>(),
        )
        .nest("/providers/gpt/accounts", account::gpt::router())
        .nest(
            "/providers/gpt/upstream-api-keys",
            provider_upstream_api_keys::router::<GptMaintenance>(),
        )
        .nest("/request-logs", statistics::request_logs_router())
        .nest("/policy-logs", statistics::policy_logs_router())
        .nest("/usage", statistics::usage_router())
        .nest("/audit-logs", statistics::audit_logs_router())
        .nest("/tenants", tenants::router())
        .nest("/users", users::router())
        // 控制台 API 未知路径必须返回 JSON 404，不能落入 SPA 的 index.html fallback。
        .fallback(console_not_found)
}

async fn console_not_found() -> impl IntoResponse {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({
            "error": {
                "code": "console_route_not_found",
                "message": "请求的控制台 API 不存在"
            }
        })),
    )
}

async fn status() -> Json<ConsoleStatusResponse<'static>> {
    Json(ConsoleStatusResponse {
        status: "ok",
        note: "console api is ready",
    })
}
