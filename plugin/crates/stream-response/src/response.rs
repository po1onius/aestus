use serde_json::Value;

use crate::{Effects, Feedback, LimitFeedback, StreamFailure, Usage};

/// 必须在修改 JSON 之前调用：response.failed 的下游瘦身会删除 `response.usage`，
/// maintenance/usage/failure 则必须依据上游原始事实生成。
pub fn effects_from_raw_json(value: &Value, status: Option<u16>, stream: bool) -> Effects {
    let error = ErrorView::from_value(value);
    Effects {
        feedback: feedback_from_response(value, status),
        usage: extract_usage(value),
        failure: (stream && is_failed_event(value)).then(|| StreamFailure {
            kind: error
                .as_ref()
                .and_then(|error| error.code.as_deref().or(error.kind.as_deref()))
                .filter(|value| !value.is_empty())
                .unwrap_or("response_failed")
                .to_owned(),
            message: error
                .as_ref()
                .map(ErrorView::reason)
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "OpenAI upstream response failed".to_owned()),
        }),
    }
}

/// 对非流式 Responses JSON 和单个 SSE data JSON 复用失败响应瘦身。正常响应中的
/// message、工具调用、图片生成结果及其他原生 Responses 字段全部保持上游原值。
pub fn transform_response_value(value: &mut Value) -> bool {
    sanitize_failed_response(value)
}

pub fn extract_usage(value: &Value) -> Option<Usage> {
    let usage = value
        .get("usage")
        .or_else(|| value.pointer("/response/usage"))?;
    let input_tokens = non_negative_i64(usage.get("input_tokens"))?;
    let output_tokens = non_negative_i64(usage.get("output_tokens"))?;
    let cached_input_tokens = non_negative_i64(
        usage
            .pointer("/input_tokens_details/cached_tokens")
            .or_else(|| usage.get("cached_input_tokens")),
    )
    .unwrap_or(0)
    .min(input_tokens);
    let reasoning_output_tokens = non_negative_i64(
        usage
            .pointer("/output_tokens_details/reasoning_tokens")
            .or_else(|| usage.get("reasoning_output_tokens")),
    )
    .unwrap_or(0)
    .min(output_tokens);
    let total_tokens = input_tokens.checked_add(output_tokens)?;
    Some(Usage {
        input_tokens,
        cached_input_tokens,
        output_tokens,
        reasoning_output_tokens,
        total_tokens,
    })
}

pub fn is_terminal_event(value: &Value) -> bool {
    matches!(
        value.get("type").and_then(Value::as_str).map(str::trim),
        Some(
            "response.completed"
                | "response.done"
                | "response.failed"
                | "response.incomplete"
                | "response.cancelled"
        )
    )
}

/// 账号额度类失败在上报原始 effects 后替换为 Codex 客户端可快速重试的终止事件。
pub fn requires_client_retry_event(value: &Value) -> bool {
    if !is_failed_event(value) {
        return false;
    }
    matches!(
        value
            .pointer("/response/error/code")
            .and_then(Value::as_str),
        Some("usage_not_included" | "insufficient_quota")
    )
}

fn is_failed_event(value: &Value) -> bool {
    value.get("type").and_then(Value::as_str) == Some("response.failed")
}

fn non_negative_i64(value: Option<&Value>) -> Option<i64> {
    let value = value?;
    let parsed = value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))?;
    (parsed >= 0).then_some(parsed)
}

fn sanitize_failed_response(value: &mut Value) -> bool {
    if !is_failed_event(value) {
        return false;
    }
    let Some(Value::Object(response)) = value.get_mut("response") else {
        return false;
    };
    let mut changed = false;
    for field in [
        "instructions",
        "output",
        "usage",
        "metadata",
        "reasoning",
        "tools",
        "tool_choice",
        "parallel_tool_calls",
        "text",
        "truncation",
        "max_output_tokens",
        "incomplete_details",
    ] {
        changed |= response.remove(field).is_some();
    }
    changed
}

#[derive(Debug)]
struct ErrorView {
    kind: Option<String>,
    code: Option<String>,
    message: Option<String>,
}

impl ErrorView {
    fn from_value(value: &Value) -> Option<Self> {
        let error = value
            .pointer("/response/error")
            .or_else(|| value.get("error"))?;
        Some(Self {
            kind: string_field(error, "type"),
            code: string_field(error, "code"),
            message: string_field(error, "message"),
        })
    }

    fn reason(&self) -> String {
        bounded_reason(
            self.message
                .as_deref()
                .or(self.code.as_deref())
                .or(self.kind.as_deref())
                .unwrap_or("OpenAI upstream request failed"),
        )
    }
}

fn string_field(value: &Value, field: &str) -> Option<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

struct SignalErrorView<'a> {
    kind: Option<&'a str>,
    code: Option<&'a str>,
    plan_type: Option<&'a str>,
    resets_at: Option<i64>,
}

impl<'a> SignalErrorView<'a> {
    /// 与网关内置 `CodexResponseError` 的 serde 解析保持一致：已知字段类型错误时整条
    /// error 都不参与资源反馈分类，未知字段则忽略。
    fn from_value(error: &'a Value) -> Option<Self> {
        error.as_object()?;
        let _message = optional_string_field(error, "message")?;
        Some(Self {
            kind: optional_string_field(error, "type")?,
            code: optional_string_field(error, "code")?,
            plan_type: optional_string_field(error, "plan_type")?,
            resets_at: optional_i64_field(error, "resets_at")?,
        })
    }
}

fn optional_string_field<'a>(value: &'a Value, field: &str) -> Option<Option<&'a str>> {
    match value.get(field) {
        None | Some(Value::Null) => Some(None),
        Some(Value::String(value)) => Some(Some(value)),
        Some(_) => None,
    }
}

fn optional_i64_field(value: &Value, field: &str) -> Option<Option<i64>> {
    match value.get(field) {
        None | Some(Value::Null) => Some(None),
        Some(value) => value
            .as_i64()
            .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
            .map(Some),
    }
}

/// 与网关内置 GPT Account 响应分类保持相同的精确规则。HTTP 失败只识别状态 401 和
/// HTTP 429 的两个 `error.type`；成功 SSE 终止事件只识别两个 `response.error.code`。
fn feedback_from_response(value: &Value, status: Option<u16>) -> Option<Feedback> {
    if status == Some(401) {
        return Some(Feedback::AuthenticationRejected("unauthorized".to_owned()));
    }

    if status == Some(429) {
        let error = SignalErrorView::from_value(value.get("error")?)?;
        return match error.kind {
            Some("usage_limit_reached") => Some(Feedback::QuotaExhausted(LimitFeedback {
                resets_at_unix_seconds: error.resets_at,
                reason: format!(
                    "usage_limit_reached: plan_type={}",
                    error.plan_type.unwrap_or("<unknown>")
                ),
            })),
            Some("usage_not_included") => Some(Feedback::EntitlementMissing(
                "usage_not_included".to_owned(),
            )),
            _ => None,
        };
    }

    if !status.is_some_and(|status| (200..300).contains(&status)) || !is_failed_event(value) {
        return None;
    }

    let error = SignalErrorView::from_value(value.pointer("/response/error")?)?;
    match error.code {
        Some("usage_not_included") => Some(Feedback::EntitlementMissing(
            "usage_not_included".to_owned(),
        )),
        Some("insufficient_quota") => Some(Feedback::QuotaExhausted(LimitFeedback {
            resets_at_unix_seconds: error.resets_at,
            reason: "quota_exhausted".to_owned(),
        })),
        _ => None,
    }
}

fn bounded_reason(reason: &str) -> String {
    const MAX_CHARS: usize = 1_024;
    let reason = reason.trim();
    if reason.chars().count() <= MAX_CHARS {
        return reason.to_owned();
    }
    reason.chars().take(MAX_CHARS).collect()
}
