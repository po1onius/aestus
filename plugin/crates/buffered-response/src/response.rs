use serde_json::Value;

use crate::{Effects, Feedback, LimitFeedback, Usage};

/// 必须在修改 JSON 之前调用：response.failed 的下游瘦身会删除 `response.usage`，
/// maintenance/usage/failure 则必须依据上游原始事实生成。
pub fn effects_from_raw_json(value: &Value, status: Option<u16>) -> Effects {
    Effects {
        feedback: feedback_from_response(value, status),
        usage: extract_usage(value),
    }
}

/// 对非流式 Responses JSON 执行下游正文转换。HTTP 429 的两个账号额度错误在原始
/// effects 提取完成后投影成统一的公开限流错误；`response.failed` 继续执行失败响应瘦身。
pub fn transform_response_value(value: &mut Value, status: u16) -> bool {
    normalize_http_rate_limit_error(value, status) | sanitize_failed_response(value)
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

fn normalize_http_rate_limit_error(value: &mut Value, status: u16) -> bool {
    if status != 429 {
        return false;
    }
    let Some(Value::Object(error)) = value.get_mut("error") else {
        return false;
    };
    if !matches!(
        error.get("type").and_then(Value::as_str),
        Some("usage_limit_reached" | "usage_not_included")
    ) {
        return false;
    }
    error.insert(
        "type".to_owned(),
        Value::String("rate_limit_exceeded".to_owned()),
    );
    error.insert(
        "message".to_owned(),
        Value::String("Rate limit reached".to_owned()),
    );
    true
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn usage_is_extracted_before_failed_body_is_sanitized() {
        let mut value = json!({
            "type": "response.failed",
            "response": {
                "error": {"code": "usage_not_included", "message": "upgrade required"},
                "output": [],
                "usage": {
                    "input_tokens": 11,
                    "output_tokens": 3,
                    "input_tokens_details": {"cached_tokens": 4},
                    "output_tokens_details": {"reasoning_tokens": 2}
                }
            }
        });
        let effects = effects_from_raw_json(&value, Some(200));
        assert_eq!(
            effects.usage,
            Some(Usage {
                input_tokens: 11,
                cached_input_tokens: 4,
                output_tokens: 3,
                reasoning_output_tokens: 2,
                total_tokens: 14,
            })
        );
        assert!(matches!(
            effects.feedback,
            Some(Feedback::EntitlementMissing(_))
        ));

        transform_response_value(&mut value, 200);
        assert!(value.pointer("/response/usage").is_none());
        assert!(value.pointer("/response/output").is_none());
        assert_eq!(
            value.pointer("/response/error/code").unwrap(),
            "usage_not_included"
        );
    }

    #[test]
    fn native_image_and_tool_items_are_preserved() {
        let duplicated = r#"{"path":"/tmp/a","old_string":"a","new_string":"b"}{"path":"/tmp/a","old_string":"a","new_string":"b"}"#;
        let mut value = json!({
            "type": "response.completed",
            "response": {
                "output": [
                    {"type":"image_generation_call","status":"generating","result":"image"},
                    {"type":"function_call","name":"apply_patch","arguments": duplicated}
                ]
            }
        });
        transform_response_value(&mut value, 200);
        assert_eq!(value["response"]["output"][0]["status"], "generating");
        assert_eq!(value["response"]["output"][1]["name"], "apply_patch");
        assert_eq!(value["response"]["output"][1]["arguments"], duplicated);
    }

    #[test]
    fn policy_failure_does_not_emit_maintenance_feedback() {
        let value = json!({
            "type":"response.failed",
            "response":{"error":{"code":"content_policy","message":"blocked"}}
        });
        let effects = effects_from_raw_json(&value, Some(200));
        assert!(effects.feedback.is_none());
    }

    #[test]
    fn untouched_payload_does_not_require_reserialization() {
        let mut value = json!({
            "type":"response.output_text.delta",
            "delta":"keep original SSE bytes"
        });
        assert!(!transform_response_value(&mut value, 200));
    }

    #[test]
    fn default_account_signals_emit_precise_feedback() {
        let mut value =
            json!({"error":{"type":"usage_limit_reached","plan_type":"plus","resets_at":123}});
        assert!(matches!(
            effects_from_raw_json(&value, Some(429)).feedback,
            Some(Feedback::QuotaExhausted(_))
        ));
        assert!(transform_response_value(&mut value, 429));
        assert_eq!(value["error"]["type"], "rate_limit_exceeded");
        assert_eq!(value["error"]["message"], "Rate limit reached");

        let mut value = json!({"error":{"type":"usage_not_included","message":"upgrade required"}});
        assert!(matches!(
            effects_from_raw_json(&value, Some(429)).feedback,
            Some(Feedback::EntitlementMissing(_))
        ));
        assert!(transform_response_value(&mut value, 429));
        assert_eq!(value["error"]["type"], "rate_limit_exceeded");
        assert_eq!(value["error"]["message"], "Rate limit reached");

        let value = json!({"error":{"message":"bad token"}});
        assert!(matches!(
            effects_from_raw_json(&value, Some(401)).feedback,
            Some(Feedback::AuthenticationRejected(_))
        ));
        let value = json!({
            "type":"response.failed",
            "response":{"error":{"code":"server_error","message":"overloaded"}}
        });
        assert!(effects_from_raw_json(&value, Some(200)).feedback.is_none());
    }
}
