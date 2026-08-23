use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use tracing::debug;

use crate::provider::{
    gpt::codex_http::response::{self as codex_response, CodexAccountSignal},
    protocol::UpstreamFeedback,
    resource::UpstreamResourceKind,
};

/// ChatGPT 账号 Responses/Images 上游返回的账号级 Codex 窗口额度 header。
///
/// 网关调用方不应观察到池内某个 OAuth 账号的剩余额度。否则一次随机调度结果会泄漏账号
/// 状态，并被客户端错误地当成网关 API Key 的稳定额度快照。
const ACCOUNT_CODEX_RATE_LIMIT_RESPONSE_HEADERS: [&str; 6] = [
    "x-codex-primary-used-percent",
    "x-codex-primary-window-minutes",
    "x-codex-primary-reset-at",
    "x-codex-secondary-used-percent",
    "x-codex-secondary-window-minutes",
    "x-codex-secondary-reset-at",
];

/// GPT 上游 HTTP 失败进入通用 executor 前的统一分类结果。
pub(super) struct HttpFailureClassification {
    pub retry: bool,
    pub exclude_resource_on_retry: bool,
    pub feedback: Option<UpstreamFeedback>,
}

/// 按资源类型统一解释 GPT HTTP 失败。
///
/// OAuth 账号可以从正文识别鉴权、额度与 entitlement 信号；Official API Key 则只按 HTTP
/// status 决定请求级重试和全局隔离。该规则由 Responses 与 Images 共用，避免两个 operation
/// 对同一个账号错误产生不同 maintenance 状态。
pub(super) fn classify_http_failure(
    resource_kind: UpstreamResourceKind,
    status: StatusCode,
    account_signal: Option<CodexAccountSignal>,
) -> HttpFailureClassification {
    if resource_kind == UpstreamResourceKind::ApiKey {
        let quarantine_key = matches!(
            status,
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN | StatusCode::TOO_MANY_REQUESTS
        );
        let retry = quarantine_key || is_transient_upstream_status(status);
        // 408 明确允许直接重试当前 Key；5xx 才在本请求后续 attempt 中切换资源，但不改变
        // Key 的全局健康状态。401/403/429 会提交全局 Key 错误回执。
        let exclude_resource_on_retry = status.is_server_error();
        debug!(
            upstream_status = status.as_u16(),
            retry,
            exclude_resource_on_retry,
            quarantine_key,
            "GPT 官方 API Key HTTP 错误已完成请求级/资源级分类"
        );
        return HttpFailureClassification {
            retry,
            exclude_resource_on_retry,
            feedback: quarantine_key.then(|| UpstreamFeedback::Error {
                reason: format!("HTTP {status}"),
            }),
        };
    }

    if let Some(signal) = account_signal {
        // usage_not_included 只说明当前账号不能承载本次调用，不足以改变账号的持久健康状态；
        // 但本请求重试时也不能再次选回同一账号。
        let exclude_resource_on_retry = matches!(&signal, CodexAccountSignal::UsageNotIncluded);
        return HttpFailureClassification {
            retry: true,
            exclude_resource_on_retry,
            feedback: Some(account_signal_to_feedback(signal)),
        };
    }

    HttpFailureClassification {
        retry: is_transient_upstream_status(status),
        exclude_resource_on_retry: false,
        feedback: None,
    }
}

pub(super) fn account_signal_to_feedback(signal: CodexAccountSignal) -> UpstreamFeedback {
    match signal {
        CodexAccountSignal::Unauthorized => UpstreamFeedback::AuthenticationRejected {
            reason: "unauthorized".to_owned(),
        },
        CodexAccountSignal::QuotaExhausted { resets_at } => UpstreamFeedback::QuotaExhausted {
            resets_at,
            reason: "quota_exhausted".to_owned(),
        },
        CodexAccountSignal::UsageLimitReached {
            plan_type,
            resets_at,
        } => UpstreamFeedback::QuotaExhausted {
            resets_at,
            reason: format!(
                "usage_limit_reached: plan_type={}",
                plan_type.as_deref().unwrap_or("<unknown>")
            ),
        },
        CodexAccountSignal::UsageNotIncluded => UpstreamFeedback::EntitlementMissing {
            reason: "usage_not_included".to_owned(),
        },
    }
}

fn is_transient_upstream_status(status: StatusCode) -> bool {
    status == StatusCode::REQUEST_TIMEOUT || status.is_server_error()
}

pub(super) fn build_upstream_url(base_url: &str, path: &str, query: Option<&str>) -> String {
    let path = if path.starts_with('/') {
        path.to_owned()
    } else {
        format!("/{path}")
    };
    let mut url = format!("{}{path}", base_url.trim().trim_end_matches('/'));
    if let Some(query) = query.filter(|value| !value.is_empty()) {
        url.push('?');
        url.push_str(query);
    }
    url
}

/// 过滤 GPT 上游响应头，并额外隐藏 OAuth 账号的真实额度窗口。
pub(super) fn filtered_response_headers(
    source: &HeaderMap,
    resource_kind: UpstreamResourceKind,
) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for (name, value) in source {
        if codex_response::should_forward_response_header(name)
            && should_forward_account_rate_limit_header(resource_kind, name)
            && let Ok(value) = HeaderValue::from_bytes(value.as_bytes())
        {
            headers.append(name.clone(), value);
        }
    }

    let filtered_account_rate_limit_headers = if resource_kind == UpstreamResourceKind::Account {
        ACCOUNT_CODEX_RATE_LIMIT_RESPONSE_HEADERS
            .iter()
            .copied()
            .filter(|name| source.contains_key(*name))
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    debug!(
        resource_type = resource_kind.as_str(),
        upstream_header_value_count = source.len(),
        downstream_header_value_count = headers.len(),
        filtered_account_rate_limit_header_count = filtered_account_rate_limit_headers.len(),
        filtered_account_rate_limit_headers = ?filtered_account_rate_limit_headers,
        "GPT 原生响应头过滤完成"
    );
    headers
}

fn should_forward_account_rate_limit_header(
    resource_kind: UpstreamResourceKind,
    name: &HeaderName,
) -> bool {
    resource_kind != UpstreamResourceKind::Account
        || !ACCOUNT_CODEX_RATE_LIMIT_RESPONSE_HEADERS.contains(&name.as_str())
}
