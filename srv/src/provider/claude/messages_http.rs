use axum::{
    body::Bytes,
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header},
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{
    err::{AppError, AppResult},
    provider::{
        claude::OAUTH_BETA,
        protocol::{
            EncodedProviderError, ProviderVisibleError, ProviderVisibleErrorKind, TokenUsage,
        },
        request_header::build_transparent_official_api_key_headers,
    },
};

pub const ANTHROPIC_VERSION: &str = "2023-06-01";
const ANTHROPIC_VERSION_HEADER: &str = "anthropic-version";
const ANTHROPIC_BETA_HEADER: &str = "anthropic-beta";
const X_APP_HEADER: &str = "x-app";
const JSON_CONTENT_TYPE: &str = "application/json";
const CLAUDE_ERROR_FALLBACK_BODY: &[u8] =
    br#"{"type":"error","error":{"type":"api_error","message":"gateway error"},"request_id":null}"#;

/// Claude 订阅账号与官方 API Key 共用路径拼接规则；Base URL 的结构只在配置或 Key
/// 导入边界校验，热路径和 maintenance 探活不再重复解析和归一化。
pub fn build_upstream_url(base_url: &str, path: &str, query: Option<&str>) -> String {
    let path = if path.starts_with('/') {
        path.to_owned()
    } else {
        format!("/{path}")
    };
    let mut url = format!("{}{path}", base_url.trim().trim_end_matches('/'));
    if let Some(query) = query.filter(|query| !query.is_empty()) {
        url.push('?');
        url.push_str(query);
    }
    url
}

pub mod request {
    use super::*;

    const ACCOUNT_UUID_FIELD: &str = "account_uuid";
    const CLIENT_USER_ID_FIELD: &str = "client_user_id";

    /// 账号级 OAuth attribution 注入后的请求体，以及不含用户原文的诊断分类。
    pub struct InjectedOauthMetadata {
        pub body: Bytes,
        pub original_user_id_kind: &'static str,
    }

    /// Messages 请求中只有少数字段需要在调度前读取；资源分配后仅最终化
    /// `metadata.user_id` 的 OAuth account UUID，其余字段保持调用方语义。
    #[derive(Debug, Clone, Deserialize)]
    pub struct MessagesRequestMetadata {
        #[serde(deserialize_with = "deserialize_required_trimmed_string")]
        pub model: String,
        #[serde(default)]
        pub metadata: Option<Metadata>,
        #[serde(default)]
        pub container: Option<Value>,
        #[serde(default, deserialize_with = "deserialize_optional_trimmed_string")]
        pub service_tier: Option<String>,
        #[serde(default, deserialize_with = "deserialize_optional_trimmed_string")]
        pub speed: Option<String>,
        #[serde(default)]
        pub thinking: Option<Value>,
    }

    impl MessagesRequestMetadata {
        /// stable API 的 container 是字符串；beta API 还允许包含 id 和 skills 的对象。
        /// 调度只关心可复用容器 ID，其他字段继续以原始 JSON 透传给 Anthropic。
        pub fn container_sticky_value(&self) -> Option<String> {
            let value = self.container.as_ref()?;
            let id = match value {
                Value::String(id) => Some(id.as_str()),
                Value::Object(container) => container.get("id").and_then(Value::as_str),
                _ => None,
            }?;
            let id = id.trim();
            (!id.is_empty()).then(|| id.to_owned())
        }
    }

    #[derive(Debug, Clone, Deserialize)]
    pub struct Metadata {
        #[serde(default, deserialize_with = "deserialize_optional_trimmed_string")]
        pub user_id: Option<String>,
    }

    pub fn parse_messages_metadata(body: &[u8]) -> AppResult<MessagesRequestMetadata> {
        serde_json::from_slice(body).map_err(|source| AppError::BadRequest {
            message: format!("Claude Messages 请求体元数据格式无效: {source}"),
        })
    }

    /// 按 Claude Code `getAPIMetadata()` 的 wire 形状注入实际调度账号 UUID。
    ///
    /// Claude Code 把 `metadata.user_id` 编码成 JSON 字符串，其中包含 `device_id`、
    /// `account_uuid`、`session_id` 和可选扩展字段。第三方 Base URL 模式下调用方无法知道
    /// 网关最终选中的订阅账号，因此必须在资源分配后覆盖 `account_uuid`。如果普通
    /// Anthropic SDK 传来的是 opaque user ID，则将它保存在 `client_user_id` 中，避免为了
    /// OAuth attribution 丢失调用方原有的滥用检测标识。
    pub fn inject_oauth_account_uuid(
        body: &[u8],
        account_uuid: uuid::Uuid,
    ) -> AppResult<InjectedOauthMetadata> {
        let mut body =
            serde_json::from_slice::<Value>(body).map_err(|source| AppError::BadRequest {
                message: format!(
                    "Claude Messages 请求体不是合法 JSON，无法注入账号 metadata: {source}"
                ),
            })?;
        let body_object = body.as_object_mut().ok_or_else(|| AppError::BadRequest {
            message: "Claude Messages 请求体必须是 JSON object，无法注入账号 metadata".to_owned(),
        })?;
        let metadata_value = body_object
            .entry("metadata")
            .or_insert_with(|| Value::Object(Map::new()));
        if metadata_value.is_null() {
            *metadata_value = Value::Object(Map::new());
        }
        let metadata = metadata_value
            .as_object_mut()
            .ok_or_else(|| AppError::BadRequest {
                message: "Claude Messages metadata 必须是 JSON object 或 null".to_owned(),
            })?;

        let (mut user_metadata, original_user_id_kind) =
            normalize_sdk_user_id(metadata.remove("user_id"))?;
        // 无论调用方是否提供或伪造 account_uuid，最终发送值都必须绑定本轮实际分配的
        // OAuth access token。其他 Claude Code metadata 字段保持原样。
        user_metadata.insert(
            ACCOUNT_UUID_FIELD.to_owned(),
            Value::String(account_uuid.to_string()),
        );
        let user_id = serde_json::to_string(&Value::Object(user_metadata)).map_err(|source| {
            AppError::BadRequest {
                message: format!("Claude Messages metadata.user_id 无法序列化: {source}"),
            }
        })?;
        metadata.insert("user_id".to_owned(), Value::String(user_id));

        let body = serde_json::to_vec(&body)
            .map(Bytes::from)
            .map_err(|source| AppError::BadRequest {
                message: format!("注入 Claude OAuth 账号 metadata 后请求体无法序列化: {source}"),
            })?;
        Ok(InjectedOauthMetadata {
            body,
            original_user_id_kind,
        })
    }

    fn normalize_sdk_user_id(
        user_id: Option<Value>,
    ) -> AppResult<(Map<String, Value>, &'static str)> {
        let Some(user_id) = user_id else {
            return Ok((Map::new(), "missing"));
        };
        match user_id {
            Value::Null => Ok((Map::new(), "null")),
            Value::String(user_id) => {
                if let Ok(Value::Object(metadata)) = serde_json::from_str::<Value>(&user_id) {
                    return Ok((metadata, "json_object_string"));
                }

                let mut metadata = Map::new();
                if !user_id.is_empty() {
                    metadata.insert(CLIENT_USER_ID_FIELD.to_owned(), Value::String(user_id));
                }
                Ok((metadata, "opaque_string"))
            }
            _ => Err(AppError::BadRequest {
                message: "Claude Messages metadata.user_id 必须是 string 或 null".to_owned(),
            }),
        }
    }

    fn deserialize_required_trimmed_string<'de, D>(deserializer: D) -> Result<String, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        let value = value.trim().to_owned();
        if value.is_empty() {
            return Err(serde::de::Error::custom("字段不能为空"));
        }
        Ok(value)
    }

    fn deserialize_optional_trimmed_string<'de, D>(
        deserializer: D,
    ) -> Result<Option<String>, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = Option::<String>::deserialize(deserializer)?;
        Ok(value
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty()))
    }
}

pub mod request_header {
    use super::*;

    /// 为 Claude OAuth 账号构造 Claude Code 兼容的基础请求头。
    ///
    /// 调用方的网关 Authorization/x-api-key 绝不能传给 Anthropic；OAuth Bearer 在最终
    /// 认证 hook 中写入。其余 Anthropic beta、客户端归因和 tracing header 可以透传，
    /// 从而保留 Claude Code 声明的协议能力。
    pub fn build_account_upstream_headers(source: &HeaderMap) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in source {
            if should_forward_request_header(name) {
                headers.append(name.clone(), value.clone());
            }
        }
        apply_managed_protocol_headers(&mut headers);
        headers
    }

    /// API Key 探活由网关生成固定 Messages 请求，不承接调用方 header，因此显式补齐探活
    /// 所需协议字段，避免把正式请求的“透明透传”语义错误套用到 maintenance。
    pub fn build_api_key_probe_headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        apply_managed_protocol_headers(&mut headers);
        headers
    }

    fn apply_managed_protocol_headers(headers: &mut HeaderMap) {
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static(JSON_CONTENT_TYPE),
        );
        headers.insert(header::ACCEPT, HeaderValue::from_static(JSON_CONTENT_TYPE));
        headers.insert(
            HeaderName::from_static(ANTHROPIC_VERSION_HEADER),
            HeaderValue::from_static(ANTHROPIC_VERSION),
        );
    }

    /// 为 Claude 官方 API Key 构造透明代理请求头。
    ///
    /// 不替调用方补充或纠正 Anthropic 协议字段，上游负责判断 `anthropic-version`、beta、
    /// Content-Type 等字段是否合法；只剥离网关凭证和无法跨连接复用的 header。
    pub fn build_official_api_key_upstream_headers(source: &HeaderMap) -> HeaderMap {
        build_transparent_official_api_key_headers(source)
    }

    pub fn inject_oauth_credential(headers: &mut HeaderMap, access_token: &str) -> AppResult<()> {
        headers.remove(header::AUTHORIZATION);
        headers.remove("x-api-key");

        append_beta_if_missing(headers, OAUTH_BETA)?;
        let bearer = format!("Bearer {}", access_token.trim());
        let mut value =
            HeaderValue::from_str(&bearer).map_err(|source| AppError::ProviderUpstream {
                provider: "claude".to_owned(),
                message: format!("Claude access token 无法写入 Authorization header: {source}"),
            })?;
        value.set_sensitive(true);
        headers.insert(header::AUTHORIZATION, value);
        Ok(())
    }

    pub fn inject_official_api_key_credential(
        headers: &mut HeaderMap,
        api_key: &str,
    ) -> AppResult<()> {
        headers.remove(header::AUTHORIZATION);
        headers.remove("x-api-key");
        // 官方 API Key 模式只替换真实凭证。包括 OAuth beta 在内的调用方协议字段与管理员
        // override 均保留，由 Anthropic 或兼容 Base URL 自行校验并返回协议错误。

        let mut value =
            HeaderValue::from_str(api_key.trim()).map_err(|source| AppError::ProviderUpstream {
                provider: "claude".to_owned(),
                message: format!("Claude 官方 API Key 无法写入 x-api-key header: {source}"),
            })?;
        value.set_sensitive(true);
        headers.insert(HeaderName::from_static("x-api-key"), value);
        Ok(())
    }

    fn append_beta_if_missing(headers: &mut HeaderMap, required: &str) -> AppResult<()> {
        let name = HeaderName::from_static(ANTHROPIC_BETA_HEADER);
        let mut values = headers
            .get_all(&name)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .flat_map(|value| value.split(','))
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if !values.iter().any(|value| value == required) {
            values.push(required.to_owned());
        }
        // Header 中可能同时存在多个值，稳定去重可保留调用方声明 beta 的原始顺序。
        let mut seen = std::collections::HashSet::new();
        values.retain(|value| seen.insert(value.clone()));
        let value =
            HeaderValue::from_str(&values.join(",")).map_err(|source| AppError::BadRequest {
                message: format!("Claude OAuth beta header 无效: {source}"),
            })?;
        headers.insert(name, value);
        Ok(())
    }

    fn should_forward_request_header(name: &HeaderName) -> bool {
        let raw = name.as_str();
        (raw == header::ACCEPT_LANGUAGE.as_str()
            || raw == ANTHROPIC_BETA_HEADER
            || raw == "anthropic-dangerous-direct-browser-access"
            || raw == header::USER_AGENT.as_str()
            || raw == X_APP_HEADER
            || raw == "anthropic-user-profile-id"
            || raw == "anthropic-workspace-id"
            || raw == "traceparent"
            || raw == "tracestate"
            || raw == "x-client-request-id"
            || raw == "x-stainless-timeout"
            || raw.starts_with("x-stainless-"))
            && !is_hop_by_hop_header(name)
    }
}

pub mod response {
    use super::*;

    #[derive(Debug, Clone, Deserialize)]
    pub struct ClaudeErrorResponse {
        #[serde(rename = "type")]
        pub response_type: Option<String>,
        pub error: ClaudeError,
        pub request_id: Option<String>,
    }

    #[derive(Debug, Clone, Deserialize)]
    pub struct ClaudeError {
        #[serde(rename = "type")]
        pub error_type: String,
        pub message: String,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum ResourceFailureSignal {
        AuthenticationRejected,
        RateLimited {
            resets_at: chrono::DateTime<chrono::Utc>,
        },
        BillingRejected,
        PermissionDenied,
    }

    pub fn parse_error(body: &[u8]) -> Option<ClaudeErrorResponse> {
        serde_json::from_slice(body).ok()
    }

    /// 从 buffered HTTP 失败响应中提取会影响当前上游资源可用性的事实。
    ///
    /// Anthropic SDK 主要按 HTTP status 构造异常，但 `billing_error` 没有独立的固定
    /// status 映射，因此必须同时读取结构化 `error.type`。认证和限流沿用 status 兜底；
    /// 403 即使正文损坏也按 permission denied 处理。billing 必须先于通用 403 判断，
    /// 才能在 `billing_error + 403` 时保留更精确的诊断类型。
    pub fn classify_resource_failure(
        status: StatusCode,
        headers: &HeaderMap,
        error: Option<&ClaudeErrorResponse>,
        fallback_cooldown_seconds: u64,
    ) -> Option<ResourceFailureSignal> {
        let error_type = error.map(|error| error.error.error_type.as_str());
        if status == StatusCode::UNAUTHORIZED || error_type == Some("authentication_error") {
            return Some(ResourceFailureSignal::AuthenticationRejected);
        }
        if status == StatusCode::TOO_MANY_REQUESTS || error_type == Some("rate_limit_error") {
            return Some(ResourceFailureSignal::RateLimited {
                resets_at: parse_rate_limit_reset(headers).unwrap_or_else(|| {
                    chrono::Utc::now()
                        + chrono::Duration::seconds(fallback_cooldown_seconds.max(1) as i64)
                }),
            });
        }
        if error_type == Some("billing_error") {
            return Some(ResourceFailureSignal::BillingRejected);
        }
        if status == StatusCode::FORBIDDEN || error_type == Some("permission_error") {
            return Some(ResourceFailureSignal::PermissionDenied);
        }
        None
    }

    pub fn is_transient(
        status: StatusCode,
        headers: &HeaderMap,
        error: Option<&ClaudeErrorResponse>,
    ) -> bool {
        if let Some(should_retry) = headers
            .get("x-should-retry")
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
        {
            if should_retry.eq_ignore_ascii_case("true") {
                return true;
            }
            if should_retry.eq_ignore_ascii_case("false") {
                return false;
            }
        }

        status == StatusCode::REQUEST_TIMEOUT
            || status == StatusCode::CONFLICT
            || status.is_server_error()
            || error.is_some_and(|error| {
                matches!(
                    error.error.error_type.as_str(),
                    "api_error" | "overloaded_error" | "timeout_error"
                )
            })
    }

    pub fn parse_rate_limit_reset(headers: &HeaderMap) -> Option<chrono::DateTime<chrono::Utc>> {
        if let Some(timestamp) = header_i64(headers, "anthropic-ratelimit-unified-reset")
            && let Some(value) = chrono::DateTime::from_timestamp(timestamp, 0)
            && value > chrono::Utc::now()
        {
            return Some(value);
        }

        // Claude 订阅账号常返回独立 5h/7d 窗口。显式 surpassed/utilization 信号优先；
        // 两个窗口都触发时选较晚 reset，避免长窗口尚未恢复却过早重新调度。
        let now = chrono::Utc::now();
        let five_hour = window_reset(headers, "5h", now);
        let seven_day = window_reset(headers, "7d", now);
        let five_hour_exceeded = window_exceeded(headers, "5h");
        let seven_day_exceeded = window_exceeded(headers, "7d");
        let window_reset = match (five_hour_exceeded, seven_day_exceeded) {
            (true, true) => latest(five_hour, seven_day),
            (true, false) => five_hour,
            (false, true) => seven_day,
            (false, false) => earliest(five_hour, seven_day),
        };
        if window_reset.is_some() {
            return window_reset;
        }

        retry_after_millis(headers)
            .and_then(|milliseconds| i64::try_from(milliseconds).ok())
            .and_then(chrono::Duration::try_milliseconds)
            .and_then(|duration| chrono::Utc::now().checked_add_signed(duration))
    }

    fn retry_after_millis(headers: &HeaderMap) -> Option<u64> {
        if let Some(milliseconds) = header_u64(headers, "retry-after-ms") {
            return Some(milliseconds.max(1));
        }
        if let Some(seconds) = header_u64(headers, "retry-after") {
            return Some(seconds.saturating_mul(1000).max(1));
        }
        let retry_at = headers
            .get("retry-after")?
            .to_str()
            .ok()
            .and_then(|value| chrono::DateTime::parse_from_rfc2822(value.trim()).ok())?
            .with_timezone(&chrono::Utc);
        let milliseconds = (retry_at - chrono::Utc::now()).num_milliseconds();
        u64::try_from(milliseconds.max(1)).ok()
    }

    fn header_i64(headers: &HeaderMap, name: &str) -> Option<i64> {
        headers.get(name)?.to_str().ok()?.trim().parse().ok()
    }

    fn header_u64(headers: &HeaderMap, name: &str) -> Option<u64> {
        headers.get(name)?.to_str().ok()?.trim().parse().ok()
    }

    fn window_reset(
        headers: &HeaderMap,
        window: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Option<chrono::DateTime<chrono::Utc>> {
        let name = format!("anthropic-ratelimit-unified-{window}-reset");
        let value = header_i64(headers, &name)
            .and_then(|value| chrono::DateTime::from_timestamp(value, 0))?;
        (value > now).then_some(value)
    }

    fn window_exceeded(headers: &HeaderMap, window: &str) -> bool {
        let prefix = format!("anthropic-ratelimit-unified-{window}-");
        let surpassed = format!("{prefix}surpassed-threshold");
        if headers
            .get(&surpassed)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("true"))
        {
            return true;
        }
        let utilization = format!("{prefix}utilization");
        headers
            .get(&utilization)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<f64>().ok())
            .is_some_and(|value| value >= 1.0 - f64::EPSILON)
    }

    fn earliest(
        left: Option<chrono::DateTime<chrono::Utc>>,
        right: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Option<chrono::DateTime<chrono::Utc>> {
        match (left, right) {
            (Some(left), Some(right)) => Some(left.min(right)),
            (left, right) => left.or(right),
        }
    }

    fn latest(
        left: Option<chrono::DateTime<chrono::Utc>>,
        right: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Option<chrono::DateTime<chrono::Utc>> {
        match (left, right) {
            (Some(left), Some(right)) => Some(left.max(right)),
            (left, right) => left.or(right),
        }
    }

    #[derive(Debug, Clone, Copy, Default, Deserialize)]
    pub struct ClaudeUsage {
        #[serde(default)]
        pub input_tokens: Option<i64>,
        #[serde(default)]
        pub cache_creation_input_tokens: Option<i64>,
        #[serde(default)]
        pub cache_read_input_tokens: Option<i64>,
        #[serde(default)]
        pub output_tokens: i64,
        #[serde(default)]
        pub output_tokens_details: Option<OutputTokensDetails>,
    }

    #[derive(Debug, Clone, Copy, Default, Deserialize)]
    pub struct OutputTokensDetails {
        #[serde(default)]
        pub thinking_tokens: i64,
    }

    impl ClaudeUsage {
        pub fn merge(self, newer: Self) -> Self {
            Self {
                input_tokens: merge_cumulative(self.input_tokens, newer.input_tokens),
                cache_creation_input_tokens: merge_cumulative(
                    self.cache_creation_input_tokens,
                    newer.cache_creation_input_tokens,
                ),
                cache_read_input_tokens: merge_cumulative(
                    self.cache_read_input_tokens,
                    newer.cache_read_input_tokens,
                ),
                output_tokens: newer.output_tokens.max(self.output_tokens),
                output_tokens_details: match (
                    self.output_tokens_details,
                    newer.output_tokens_details,
                ) {
                    (Some(current), Some(newer)) => Some(OutputTokensDetails {
                        thinking_tokens: current.thinking_tokens.max(newer.thinking_tokens),
                    }),
                    (current, newer) => newer.or(current),
                },
            }
        }

        pub fn into_token_usage(self) -> TokenUsage {
            // 上游 token 计数理论上均非负；归零保护避免异常响应反向增加用户额度。
            let cached_input_tokens = self.cache_read_input_tokens.unwrap_or_default().max(0);
            let input_tokens = self
                .input_tokens
                .unwrap_or_default()
                .max(0)
                .saturating_add(self.cache_creation_input_tokens.unwrap_or_default().max(0))
                .saturating_add(cached_input_tokens);
            let reasoning_output_tokens = self
                .output_tokens_details
                .map_or(0, |details| details.thinking_tokens)
                .max(0);
            let output_tokens = self.output_tokens.max(0);
            TokenUsage {
                input_tokens,
                cached_input_tokens,
                output_tokens,
                reasoning_output_tokens,
                total_tokens: input_tokens.saturating_add(output_tokens),
            }
        }
    }

    fn merge_cumulative(current: Option<i64>, newer: Option<i64>) -> Option<i64> {
        match (current, newer) {
            (Some(current), Some(newer)) => Some(current.max(newer)),
            (current, newer) => newer.or(current),
        }
    }

    #[derive(Debug, Deserialize)]
    struct MessageResponseUsage {
        usage: ClaudeUsage,
    }

    #[derive(Debug, Deserialize)]
    struct CountTokensResponse {
        input_tokens: i64,
    }

    #[derive(Debug, Deserialize)]
    struct SseEnvelope {
        #[serde(rename = "type")]
        event_type: Option<String>,
        message: Option<MessageResponseUsage>,
        usage: Option<ClaudeUsage>,
        error: Option<ClaudeError>,
    }

    pub enum SseData {
        Usage(ClaudeUsage),
        Error(ClaudeError),
        Other,
    }

    pub fn parse_non_stream_usage(body: &[u8]) -> Option<ClaudeUsage> {
        serde_json::from_slice::<MessageResponseUsage>(body)
            .ok()
            .map(|message| message.usage)
    }

    pub fn parse_count_tokens(body: &[u8]) -> Option<i64> {
        serde_json::from_slice::<CountTokensResponse>(body)
            .ok()
            .map(|response| response.input_tokens)
            .filter(|input_tokens| *input_tokens >= 0)
    }

    pub fn parse_sse_data(data: &[u8]) -> Option<SseData> {
        let envelope = serde_json::from_slice::<SseEnvelope>(data).ok()?;
        match envelope.event_type.as_deref() {
            Some("message_start") => envelope
                .message
                .map(|message| SseData::Usage(message.usage))
                .or(Some(SseData::Other)),
            Some("message_delta") => envelope.usage.map(SseData::Usage).or(Some(SseData::Other)),
            Some("error") => envelope.error.map(SseData::Error),
            _ => Some(SseData::Other),
        }
    }

    pub fn collect_sse_event_data(event: &[u8]) -> Option<Vec<u8>> {
        let text = std::str::from_utf8(event).ok()?;
        let mut data = String::new();
        for line in text.lines() {
            let Some(value) = line.strip_prefix("data:") else {
                continue;
            };
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(value.strip_prefix(' ').unwrap_or(value));
        }
        (!data.is_empty()).then(|| data.into_bytes())
    }

    pub fn find_sse_event_boundary(buffer: &[u8]) -> Option<(usize, usize)> {
        buffer
            .windows(2)
            .position(|window| window == b"\n\n")
            .map(|index| (index, 2))
            .or_else(|| {
                buffer
                    .windows(4)
                    .position(|window| window == b"\r\n\r\n")
                    .map(|index| (index, 4))
            })
    }

    /// 把通用 gateway 已脱敏的错误编码为 Anthropic 原生错误响应。
    ///
    /// 返回的 `EncodedProviderError::body` 同时供 ClickHouse 与实际 HTTP response 使用，
    /// 因而这里只允许序列化一次；真实上游错误响应不经过本函数。
    pub fn encode_provider_error(
        error: &ProviderVisibleError,
        request_id: uuid::Uuid,
    ) -> EncodedProviderError {
        let error_type = match error.kind {
            ProviderVisibleErrorKind::Authentication => "authentication_error",
            ProviderVisibleErrorKind::Permission => "permission_error",
            ProviderVisibleErrorKind::InvalidRequest => "invalid_request_error",
            ProviderVisibleErrorKind::RateLimit => "rate_limit_error",
            ProviderVisibleErrorKind::Gateway => "api_error",
        };
        let payload = ProviderErrorResponse {
            response_type: "error",
            error: ProviderErrorBody {
                error_type,
                message: error.message.clone(),
            },
            request_id: Some(request_id.to_string()),
        };
        let (status, body) = match serde_json::to_vec(&payload) {
            Ok(body) => (error.status, Bytes::from(body)),
            Err(source) => {
                tracing::error!(
                    request_id = %request_id,
                    error = %source,
                    "Claude provider 可见错误序列化失败，使用 gateway error 固定响应"
                );
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Bytes::from_static(CLAUDE_ERROR_FALLBACK_BODY),
                )
            }
        };
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static(JSON_CONTENT_TYPE),
        );
        match HeaderValue::from_str(&request_id.to_string()) {
            Ok(value) => {
                headers.insert("request-id", value);
            }
            Err(source) => {
                tracing::error!(
                    request_id = %request_id,
                    error = %source,
                    "Claude gateway request_id 无法编码为响应头"
                );
            }
        }

        EncodedProviderError {
            status,
            headers,
            body,
        }
    }

    #[derive(Debug, Serialize)]
    struct ProviderErrorResponse {
        #[serde(rename = "type")]
        response_type: &'static str,
        error: ProviderErrorBody,
        request_id: Option<String>,
    }

    #[derive(Debug, Serialize)]
    struct ProviderErrorBody {
        #[serde(rename = "type")]
        error_type: &'static str,
        message: String,
    }

    pub fn copy_response_headers(source: &HeaderMap, target: &mut HeaderMap) {
        for (name, value) in source {
            if should_forward_response_header(name)
                && let Ok(value) = HeaderValue::from_bytes(value.as_bytes())
            {
                target.append(name.clone(), value);
            }
        }
    }

    fn should_forward_response_header(name: &HeaderName) -> bool {
        !is_hop_by_hop_header(name) && name != header::CONTENT_LENGTH
    }
}

fn is_hop_by_hop_header(name: &HeaderName) -> bool {
    name == header::CONNECTION
        || name == header::TRANSFER_ENCODING
        || name == header::TE
        || name == header::TRAILER
        || name == header::UPGRADE
        || name.as_str() == "keep-alive"
        || name.as_str() == "proxy-authenticate"
        || name.as_str() == "proxy-authorization"
}
