//! 控制台 HTTP 审计采集。只读取请求元数据，不消费正文、不改变鉴权结果。

use std::{
    net::SocketAddr,
    sync::{Arc, OnceLock},
    time::Instant,
};

use axum::{
    extract::{ConnectInfo, OriginalUri, Request, State},
    http::header::USER_AGENT,
    middleware::Next,
    response::Response,
};
use chrono::Utc;
use tracing::debug;
use uuid::Uuid;

use crate::{
    logs::audit::{AuditActor, AuditLogRecord},
    state::AppState,
    user::User,
};

/// middleware 与鉴权 extractor/登录 handler 共享，身份只记录一次，无额外数据库查询。
#[derive(Clone, Default)]
pub(crate) struct AuditContext(Arc<OnceLock<AuditActor>>);

impl AuditContext {
    /// 仅在凭证已验证或注册已成功后调用；角色/启停校验拒绝也保留已确认身份。
    pub(crate) fn record_user(&self, user: &User) {
        let _ = self.0.set(AuditActor {
            user_id: user.id,
            username: user.username.clone(),
            role: user.role.clone(),
            tenant_id: user.tenant_id.clone(),
        });
    }
}

pub(crate) async fn record_request(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    let id = Uuid::now_v7();
    let occurred_at = Utc::now();
    let started = Instant::now();
    // OriginalUri 保留嵌套路由前的完整路径；仅 path，永远不采集 query。
    let path = request
        .extensions()
        .get::<OriginalUri>()
        .map(|uri| uri.0.path())
        .unwrap_or_else(|| request.uri().path())
        .to_owned();
    let method = request.method().as_str().to_owned();
    // 关联 ID 可能由客户端提供，不能替代服务端生成的审计主键。
    let request_id = request
        .headers()
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .map(|value| value.chars().take(128).collect());
    let user_agent = request
        .headers()
        .get(USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.chars().take(512).collect());
    // 只信任连接信息；反向代理部署时这里明确记录代理地址，不信任客户端伪造的转发头。
    let peer_ip = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|peer| peer.0.ip().to_string());
    let context = AuditContext::default();
    request.extensions_mut().insert(context.clone());
    let response = next.run(request).await;
    let actor = context.0.get();
    let status_code = i32::from(response.status().as_u16());
    let duration_ms = i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX);
    debug!(audit_id = %id, status_code, duration_ms, authenticated = actor.is_some(), "控制台请求审计采集完成");
    state.audit_logs().emit(AuditLogRecord {
        id,
        occurred_at,
        request_id,
        user_id: actor.map(|actor| actor.user_id),
        username: actor.map(|actor| actor.username.clone()),
        role: actor.map(|actor| actor.role.clone()),
        tenant_id: actor.and_then(|actor| actor.tenant_id.clone()),
        method,
        path,
        status_code,
        duration_ms,
        peer_ip,
        user_agent,
    });
    response
}
