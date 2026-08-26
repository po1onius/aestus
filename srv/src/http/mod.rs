pub mod dash;
pub mod gateway;

use axum::{
    Json, Router,
    extract::DefaultBodyLimit,
    http::{
        HeaderValue, Method,
        header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE},
    },
    routing::get,
};
use serde::Serialize;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::{
    cors::CorsLayer,
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    trace::TraceLayer,
};
use tracing::Level;

use crate::state::AppState;

const REQUEST_ID_HEADER: &str = "x-request-id";
const DASHBOARD_MAX_BODY_BYTES: usize = 256 * 1024;

#[derive(Debug, Serialize)]
struct HealthResponse<'a> {
    status: &'a str,
    service: &'a str,
}

/// 构建 HTTP 路由。
///
/// 网关接口和管理面板接口分开挂载，避免管理面板鉴权、中间件和请求边界策略
/// 影响 OpenAI 兼容接口的行为。
pub fn build_router(state: AppState) -> Router {
    let cors = CorsLayer::new()
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
        .allow_headers([
            AUTHORIZATION,
            CONTENT_TYPE,
            ACCEPT,
            "x-api-key".parse().expect("请求头名称固定有效"),
            "anthropic-version".parse().expect("请求头名称固定有效"),
            "anthropic-beta".parse().expect("请求头名称固定有效"),
            "anthropic-dangerous-direct-browser-access"
                .parse()
                .expect("请求头名称固定有效"),
            "anthropic-user-profile-id"
                .parse()
                .expect("请求头名称固定有效"),
            "anthropic-workspace-id"
                .parse()
                .expect("请求头名称固定有效"),
            "traceparent".parse().expect("请求头名称固定有效"),
            "tracestate".parse().expect("请求头名称固定有效"),
            // Codex standalone search 会把线程来源和 turn 上下文放在这两个 header 中；
            // 浏览器类客户端跨域调用搜索接口时需要允许它们通过预检。
            "originator".parse().expect("请求头名称固定有效"),
            "x-codex-turn-metadata".parse().expect("请求头名称固定有效"),
            "x-app".parse().expect("请求头名称固定有效"),
            "x-client-request-id".parse().expect("请求头名称固定有效"),
            "x-stainless-arch".parse().expect("请求头名称固定有效"),
            "x-stainless-helper".parse().expect("请求头名称固定有效"),
            "x-stainless-helper-method"
                .parse()
                .expect("请求头名称固定有效"),
            "x-stainless-lang".parse().expect("请求头名称固定有效"),
            "x-stainless-os".parse().expect("请求头名称固定有效"),
            "x-stainless-package-version"
                .parse()
                .expect("请求头名称固定有效"),
            "x-stainless-retry-count"
                .parse()
                .expect("请求头名称固定有效"),
            "x-stainless-runtime".parse().expect("请求头名称固定有效"),
            "x-stainless-runtime-version"
                .parse()
                .expect("请求头名称固定有效"),
            "x-stainless-timeout".parse().expect("请求头名称固定有效"),
        ])
        .allow_origin(HeaderValue::from_static("*"));

    // Dashboard 只接收表单和资源配置，不继承 LLM 上传场景的 64 MiB 上限；跨域能力也只
    // 授予对外网关协议，避免任意站点直接调用管理端 Bearer API。
    let gateway_router = gateway::router().layer(cors);
    let dashboard_router = dash::router().layer(DefaultBodyLimit::max(DASHBOARD_MAX_BODY_BYTES));
    let router = Router::new()
        .route("/healthz", get(healthz))
        .merge(gateway_router)
        .nest("/dash", dashboard_router);
    let router = mount_web_dist_if_configured(router, &state);

    router
        .with_state(state)
        .layer(PropagateRequestIdLayer::new(
            REQUEST_ID_HEADER.parse().expect("请求头名称固定有效"),
        ))
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(tower_http::trace::DefaultMakeSpan::new().level(Level::INFO))
                .on_response(tower_http::trace::DefaultOnResponse::new().level(Level::INFO)),
        )
}

fn mount_web_dist_if_configured(router: Router<AppState>, state: &AppState) -> Router<AppState> {
    let Some(web_dist_dir) = state.config().web_dist_dir.as_deref() else {
        return router;
    };

    tracing::info!(web_dist_dir, "已启用前端静态资源托管");

    router.fallback_service(
        ServeDir::new(web_dist_dir).fallback(ServeFile::new(format!("{web_dist_dir}/index.html"))),
    )
}

async fn healthz() -> Json<HealthResponse<'static>> {
    Json(HealthResponse {
        status: "ok",
        service: "aestus-gateway",
    })
}
