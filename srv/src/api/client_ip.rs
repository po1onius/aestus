//! 控制台共用的来源 IP 上下文：审计和限流只消费同一次可信代理解析结果。

use std::net::{IpAddr, SocketAddr};

use axum::{
    extract::{ConnectInfo, Request, State},
    http::HeaderMap,
    middleware::Next,
    response::Response,
};
use axum_client_addr::{ClientIpConfig, ClientIpSource};
use tracing::{debug, error, warn};

use crate::state::AppState;

#[derive(Clone, Copy, Default)]
pub(super) struct RequestClientIp {
    pub peer_ip: Option<IpAddr>,
    pub client_ip: Option<IpAddr>,
}

pub(super) async fn resolve_request_ip(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    let peer_ip = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|peer| peer.0.ip());
    let client_ip = peer_ip.map(|peer| {
        resolve_client_ip(request.headers(), peer, &state.config().client_ip_config).to_canonical()
    });
    if peer_ip.is_none() {
        error!("控制台请求缺少 TCP 连接信息，请检查 HTTP 服务是否启用 ConnectInfo<SocketAddr>");
    }
    request
        .extensions_mut()
        .insert(RequestClientIp { peer_ip, client_ip });
    next.run(request).await
}

fn resolve_client_ip(headers: &HeaderMap, peer_ip: IpAddr, config: &ClientIpConfig) -> IpAddr {
    if !config.is_trusted_proxy(peer_ip) {
        debug!(%peer_ip, "对端未命中可信代理，请求来源使用 TCP IP");
        return peer_ip;
    }
    // 库按原始顺序处理多行 XFF，从右向左剥离可信代理；遇到无法识别的 hop
    // 就停止，不跳过未知 hop 去信任更左侧的数据。配置禁用了其他代理头。
    let resolved = config.resolve_client_ip(headers, peer_ip);
    if matches!(resolved.source(), ClientIpSource::ChainHeader(_)) {
        debug!(%peer_ip, client_ip = %resolved.ip(), "已通过可信代理 XFF 确定请求来源 IP");
        resolved.ip()
    } else {
        warn!(%peer_ip, xff_present = headers.contains_key("x-forwarded-for"), "可信代理 XFF 缺失或没有可用来源地址，使用 TCP 对端 IP");
        peer_ip
    }
}
