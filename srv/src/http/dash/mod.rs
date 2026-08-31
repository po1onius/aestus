mod account;
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
    provider::{claude::maintenance::ClaudeMaintenance, gpt::maintenance::GptMaintenance},
    state::AppState,
};

#[derive(Debug, Serialize)]
struct DashStatusResponse<'a> {
    status: &'a str,
    note: &'a str,
}

/// 管理面板 API 路由。
///
/// 这里先提供一个占位状态接口，后续账号管理、API Key 管理、监控查询都挂到该模块下。
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/status", get(status))
        .nest("/auth", auth::router())
        .nest("/api-keys", gateway_api_keys::router())
        .nest("/plugins", plugins::router())
        .nest("/provider-groups", provider_groups::router())
        .nest("/claude-accounts", account::claude::router())
        .nest(
            "/claude-upstream-api-keys",
            provider_upstream_api_keys::router::<ClaudeMaintenance>(),
        )
        .nest("/gpt-accounts", account::gpt::router())
        .nest(
            "/gpt-upstream-api-keys",
            provider_upstream_api_keys::router::<GptMaintenance>(),
        )
        .nest("/request-logs", statistics::request_logs_router())
        .nest("/usage", statistics::usage_router())
        .nest("/tenants", tenants::router())
        .nest("/users", users::router())
        // `/dash` 未知路径必须返回 JSON 404，不能继续落入 SPA 的 index.html fallback。
        .fallback(dash_not_found)
}

async fn dash_not_found() -> impl IntoResponse {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({
            "error": {
                "code": "dashboard_route_not_found",
                "message": "请求的 Dashboard API 不存在"
            }
        })),
    )
}

async fn status() -> Json<DashStatusResponse<'static>> {
    Json(DashStatusResponse {
        status: "ok",
        note: "dashboard api is ready",
    })
}
