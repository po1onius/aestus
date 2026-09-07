//! GPT OAuth 账号原生响应的策略日志识别，仅返回日志事实，不参与响应或 maintenance 决策。

use axum::http::StatusCode;
use chrono::Utc;
use serde_json::Value;

use crate::{
    provider::resource::UpstreamResourceKind,
    request::events::{GptPolicyErrorCode, GptPolicyViolation},
};

pub(super) fn parse_http_error(
    resource_kind: UpstreamResourceKind,
    status: StatusCode,
    body: &[u8],
) -> Option<GptPolicyViolation> {
    if resource_kind != UpstreamResourceKind::Account
        || !matches!(status, StatusCode::BAD_REQUEST | StatusCode::FORBIDDEN)
    {
        return None;
    }
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return None;
    };
    let code = value.pointer("/error/code").and_then(Value::as_str);
    let error_code = match (status, code) {
        (StatusCode::BAD_REQUEST, Some("cyber_policy")) => GptPolicyErrorCode::CyberPolicy,
        (_, Some("misalignment_policy_violation")) => {
            GptPolicyErrorCode::MisalignmentPolicyViolation
        }
        _ => return None,
    };
    Some(GptPolicyViolation {
        occurred_at: Utc::now(),
        error_code,
    })
}

/// 调用方已经确认这是 response.failed，HTTP 状态与 error.type 不参与 SSE 判断。
pub(super) fn parse_stream_error_code(
    resource_kind: UpstreamResourceKind,
    code: Option<&str>,
) -> Option<GptPolicyViolation> {
    if resource_kind != UpstreamResourceKind::Account {
        return None;
    }
    let error_code = match code {
        Some("cyber_policy") => GptPolicyErrorCode::CyberPolicy,
        Some("misalignment_policy_violation") => GptPolicyErrorCode::MisalignmentPolicyViolation,
        Some("bio_policy") => GptPolicyErrorCode::BioPolicy,
        _ => return None,
    };
    Some(GptPolicyViolation {
        occurred_at: Utc::now(),
        error_code,
    })
}
