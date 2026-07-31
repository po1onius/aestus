use std::collections::HashSet;

use axum::http::{HeaderMap, HeaderName, header};
use tracing::debug;

/// 为官方 API Key 资源构造尽量透明的上游请求头。
///
/// 网关会终止调用方连接并重新建立上游连接，因此只剥离以下无法跨连接复用的字段：
/// - 调用方用于访问网关的认证字段，避免网关 Key 泄漏到 provider；
/// - hop-by-hop 及 `Connection` 动态声明的连接级字段；
/// - `Host`、`Content-Length` 等必须按实际上游 URL/body 重新生成的 framing 字段。
///
/// 其余 end-to-end header 不使用白名单，完整保留调用方提供的多值及未来协议扩展。资源级
/// header override 会在本函数之后应用，因此管理员仍可显式施加资源配置；真实 provider
/// 凭证则由最后的请求最终化 hook 覆盖写入。
pub(crate) fn build_transparent_official_api_key_headers(source: &HeaderMap) -> HeaderMap {
    let connection_scoped_headers = connection_scoped_header_names(source);
    let mut upstream = HeaderMap::new();
    let mut filtered_names = Vec::new();

    for (name, value) in source {
        if should_forward(name, &connection_scoped_headers) {
            upstream.append(name.clone(), value.clone());
        } else {
            filtered_names.push(name.as_str().to_owned());
        }
    }

    filtered_names.sort_unstable();
    filtered_names.dedup();
    debug!(
        source_header_value_count = source.len(),
        upstream_header_value_count = upstream.len(),
        filtered_header_names = ?filtered_names,
        "官方 API Key 请求已透明透传 end-to-end header，并剥离网关认证及连接级 header"
    );
    upstream
}

fn should_forward(name: &HeaderName, connection_scoped_headers: &HashSet<HeaderName>) -> bool {
    !is_gateway_credential(name)
        && name != header::HOST
        && name != header::CONTENT_LENGTH
        && !is_standard_hop_by_hop_header(name)
        && !connection_scoped_headers.contains(name)
}

fn is_gateway_credential(name: &HeaderName) -> bool {
    name == header::AUTHORIZATION || name.as_str() == "x-api-key"
}

fn is_standard_hop_by_hop_header(name: &HeaderName) -> bool {
    name == header::CONNECTION
        || name == header::TRANSFER_ENCODING
        || name == header::TE
        || name == header::TRAILER
        || name == header::UPGRADE
        || name.as_str() == "keep-alive"
        || name.as_str() == "proxy-authenticate"
        || name.as_str() == "proxy-authorization"
        || name.as_str() == "proxy-connection"
}

/// HTTP/1.1 允许 `Connection: foo, bar` 再动态声明任意连接级字段。即使 `foo`、`bar`
/// 本身不是标准 hop-by-hop header，代理也不能把它们带到下一跳。
fn connection_scoped_header_names(source: &HeaderMap) -> HashSet<HeaderName> {
    source
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .filter_map(|name| HeaderName::from_bytes(name.as_bytes()).ok())
        .collect()
}
