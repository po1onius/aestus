pub mod request {
    use std::str::FromStr;

    use base64::Engine;
    use chrono::{DateTime, Utc};
    use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
    use tracing::debug;

    use crate::err::{AppError, AppResult};

    const CODEX_TURN_METADATA_KEY: &str = "x-codex-turn-metadata";
    const COMPACTION_REQUEST_KIND: &str = "compaction";

    #[derive(Debug, Clone)]
    pub struct CodexIdTokenClaims {
        pub email: Option<String>,
        pub chatgpt_plan_type: Option<String>,
        pub chatgpt_user_id: Option<String>,
        pub chatgpt_account_id: Option<String>,
        pub chatgpt_account_is_fedramp: bool,
    }

    #[derive(Debug, Deserialize)]
    struct IdTokenClaims {
        #[serde(default)]
        email: Option<String>,
        #[serde(rename = "https://api.openai.com/profile", default)]
        profile: Option<ProfileClaims>,
        #[serde(rename = "https://api.openai.com/auth", default)]
        auth: Option<ChatGptAuthClaims>,
    }

    #[derive(Debug, Deserialize)]
    struct ProfileClaims {
        #[serde(default)]
        email: Option<String>,
    }

    #[derive(Debug, Deserialize)]
    struct ChatGptAuthClaims {
        #[serde(default)]
        chatgpt_plan_type: Option<CodexPlanType>,
        #[serde(default)]
        chatgpt_user_id: Option<String>,
        #[serde(default)]
        user_id: Option<String>,
        #[serde(default)]
        chatgpt_account_id: Option<String>,
        #[serde(default)]
        chatgpt_account_is_fedramp: bool,
    }

    #[derive(Debug, Deserialize)]
    struct StandardJwtClaims {
        #[serde(default)]
        exp: Option<i64>,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum CodexPlanType {
        Known(KnownCodexPlan),
        Unknown(String),
    }

    impl CodexPlanType {
        fn from_raw_value(raw: &str) -> Self {
            match raw.to_ascii_lowercase().as_str() {
                "free" => Self::Known(KnownCodexPlan::Free),
                "go" => Self::Known(KnownCodexPlan::Go),
                "plus" => Self::Known(KnownCodexPlan::Plus),
                "pro" => Self::Known(KnownCodexPlan::Pro),
                "prolite" => Self::Known(KnownCodexPlan::ProLite),
                "team" => Self::Known(KnownCodexPlan::Team),
                "self_serve_business_usage_based" => {
                    Self::Known(KnownCodexPlan::SelfServeBusinessUsageBased)
                }
                "business" => Self::Known(KnownCodexPlan::Business),
                "enterprise_cbp_usage_based" => {
                    Self::Known(KnownCodexPlan::EnterpriseCbpUsageBased)
                }
                "enterprise" | "hc" => Self::Known(KnownCodexPlan::Enterprise),
                "education" | "edu" => Self::Known(KnownCodexPlan::Edu),
                _ => Self::Unknown(raw.to_owned()),
            }
        }

        fn raw_value(&self) -> String {
            match self {
                Self::Known(plan) => plan.raw_value().to_owned(),
                Self::Unknown(plan) => plan.clone(),
            }
        }
    }

    impl<'de> Deserialize<'de> for CodexPlanType {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: Deserializer<'de>,
        {
            let raw = String::deserialize(deserializer)?;
            Ok(Self::from_raw_value(&raw))
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum KnownCodexPlan {
        Free,
        Go,
        Plus,
        Pro,
        ProLite,
        Team,
        SelfServeBusinessUsageBased,
        Business,
        EnterpriseCbpUsageBased,
        Enterprise,
        Edu,
    }

    impl KnownCodexPlan {
        fn raw_value(self) -> &'static str {
            match self {
                Self::Free => "free",
                Self::Go => "go",
                Self::Plus => "plus",
                Self::Pro => "pro",
                Self::ProLite => "prolite",
                Self::Team => "team",
                Self::SelfServeBusinessUsageBased => "self_serve_business_usage_based",
                Self::Business => "business",
                Self::EnterpriseCbpUsageBased => "enterprise_cbp_usage_based",
                Self::Enterprise => "enterprise",
                Self::Edu => "edu",
            }
        }
    }

    /// 解析 Codex 登录产生的 JWT payload。
    ///
    /// 官方 Codex 会从 id_token 的 `https://api.openai.com/auth` claims 中读取
    /// `chatgpt_plan_type`、`chatgpt_user_id/user_id`、`chatgpt_account_id` 和
    /// `chatgpt_account_is_fedramp`，这里保持同样的字段语义和 plan alias 归一化规则。
    pub fn parse_id_token_claims(id_token: &str) -> Result<CodexIdTokenClaims, String> {
        let claims = decode_jwt_payload::<IdTokenClaims>(id_token)?;
        let email = claims
            .email
            .or_else(|| claims.profile.and_then(|profile| profile.email))
            .and_then(normalize_claim_string);
        let auth = claims.auth.unwrap_or(ChatGptAuthClaims {
            chatgpt_plan_type: None,
            chatgpt_user_id: None,
            user_id: None,
            chatgpt_account_id: None,
            chatgpt_account_is_fedramp: false,
        });
        let chatgpt_plan_type = auth
            .chatgpt_plan_type
            .map(|plan| plan.raw_value())
            .and_then(normalize_claim_string);
        let chatgpt_user_id = auth
            .chatgpt_user_id
            .or(auth.user_id)
            .and_then(normalize_claim_string);
        let chatgpt_account_id = auth.chatgpt_account_id.and_then(normalize_claim_string);

        Ok(CodexIdTokenClaims {
            email,
            chatgpt_plan_type,
            chatgpt_user_id,
            chatgpt_account_id,
            chatgpt_account_is_fedramp: auth.chatgpt_account_is_fedramp,
        })
    }

    pub fn parse_jwt_expiration(jwt: &str) -> Result<Option<DateTime<Utc>>, String> {
        let claims = decode_jwt_payload::<StandardJwtClaims>(jwt)?;
        Ok(claims
            .exp
            .and_then(|timestamp| DateTime::<Utc>::from_timestamp(timestamp, 0)))
    }

    fn decode_jwt_payload<T>(jwt: &str) -> Result<T, String>
    where
        T: serde::de::DeserializeOwned,
    {
        let mut parts = jwt.split('.');
        let (_header, payload, _signature) = match (parts.next(), parts.next(), parts.next()) {
            (Some(header), Some(payload), Some(signature))
                if !header.is_empty() && !payload.is_empty() && !signature.is_empty() =>
            {
                (header, payload, signature)
            }
            _ => return Err("JWT 格式无效".to_owned()),
        };

        let payload_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(payload)
            .map_err(|source| format!("JWT payload base64 解码失败: {source}"))?;

        serde_json::from_slice::<T>(&payload_bytes)
            .map_err(|source| format!("JWT payload JSON 解析失败: {source}"))
    }

    fn normalize_claim_string(value: String) -> Option<String> {
        let value = value.trim().to_owned();
        (!value.is_empty()).then_some(value)
    }

    /// 官方 Codex `reasoning.context` 枚举，按 snake_case 出现在 JSON 中。
    #[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
    #[serde(rename_all = "snake_case")]
    pub enum ReasoningContext {
        Auto,
        CurrentTurn,
        AllTurns,
    }

    /// 官方 Codex `reasoning.summary` 枚举，按小写字符串出现在 JSON 中。
    #[derive(Debug, Serialize, Deserialize, Default, Clone, Copy, PartialEq, Eq)]
    #[serde(rename_all = "lowercase")]
    pub enum ReasoningSummary {
        #[default]
        Auto,
        Concise,
        Detailed,
        None,
    }

    /// 官方 Codex `reasoning.effort`，保留未知自定义字符串，避免官方新增枚举值时
    /// 网关入口误拒绝仍可透传的请求。
    #[derive(Debug, Default, Clone, PartialEq, Eq, Hash)]
    pub enum ReasoningEffort {
        None,
        Minimal,
        Low,
        #[default]
        Medium,
        High,
        XHigh,
        Ultra,
        Custom(String),
    }

    impl ReasoningEffort {
        pub fn as_str(&self) -> &str {
            match self {
                Self::None => "none",
                Self::Minimal => "minimal",
                Self::Low => "low",
                Self::Medium => "medium",
                Self::High => "high",
                Self::XHigh => "xhigh",
                Self::Ultra => "ultra",
                Self::Custom(effort) => effort,
            }
        }
    }

    impl Serialize for ReasoningEffort {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            serializer.serialize_str(self.as_str())
        }
    }

    impl<'de> Deserialize<'de> for ReasoningEffort {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: Deserializer<'de>,
        {
            let effort = String::deserialize(deserializer)?;
            effort.parse().map_err(de::Error::custom)
        }
    }

    impl FromStr for ReasoningEffort {
        type Err = String;

        fn from_str(value: &str) -> Result<Self, Self::Err> {
            match value {
                "none" => Ok(Self::None),
                "minimal" => Ok(Self::Minimal),
                "low" => Ok(Self::Low),
                "medium" => Ok(Self::Medium),
                "high" => Ok(Self::High),
                "xhigh" => Ok(Self::XHigh),
                "ultra" => Ok(Self::Ultra),
                "" => Err("reasoning_effort must not be empty".to_owned()),
                effort => Ok(Self::Custom(effort.to_owned())),
            }
        }
    }

    /// 官方 Codex Responses API 的 reasoning 控制块。
    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    pub struct Reasoning {
        #[serde(skip_serializing_if = "Option::is_none")]
        pub effort: Option<ReasoningEffort>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub summary: Option<ReasoningSummary>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub context: Option<ReasoningContext>,
    }

    /// GPT Responses adapter 需要的协议元数据。
    ///
    /// 这些字段属于 GPT/Codex 协议语义，不能放在通用 gateway 层解析。adapter 通过
    /// 该结构提取模型授权、调度粘性和日志字段，同时原始请求体仍继续透传。
    #[derive(Debug, Clone, Deserialize)]
    pub struct ResponsesRequestMetadata {
        #[serde(deserialize_with = "deserialize_required_trimmed_string")]
        pub model: String,
        #[serde(default)]
        pub reasoning: Option<Reasoning>,
        #[serde(default, deserialize_with = "deserialize_optional_trimmed_string")]
        pub service_tier: Option<String>,
        #[serde(default, deserialize_with = "deserialize_optional_trimmed_string")]
        pub prompt_cache_key: Option<String>,
        /// Codex 把 turn metadata 作为 `client_metadata` 中的 JSON 字符串发送。这里只投影
        /// 压缩分类，不保留完整 metadata，避免把调用方诊断信息复制到请求日志。
        #[serde(
            rename = "client_metadata",
            default,
            deserialize_with = "deserialize_is_compaction"
        )]
        pub is_compaction: bool,
    }

    /// Codex turn metadata 中与请求日志分类有关的最小投影。
    #[derive(Debug, Deserialize)]
    struct CodexTurnMetadata {
        #[serde(default)]
        request_kind: Option<String>,
    }

    /// 解析 GPT Responses adapter 需要的私有元数据。
    pub fn parse_responses_metadata(body: &[u8]) -> AppResult<ResponsesRequestMetadata> {
        serde_json::from_slice::<ResponsesRequestMetadata>(body).map_err(|source| {
            AppError::BadRequest {
                message: format!("GPT Responses 请求体元数据格式无效: {source}"),
            }
        })
    }

    /// 从 Codex canonical client metadata 中识别压缩请求。
    ///
    /// `client_metadata["x-codex-turn-metadata"]` 的 wire value 本身是一个 JSON 字符串。
    /// 该字段只用于可观测性，格式异常不能让原本可执行的模型请求失败，因此任何缺失、类型
    /// 不匹配或嵌套 JSON 解析失败都降级为普通 GPT 请求，并且日志只记录结构和错误，不记录
    /// 完整 metadata 正文。
    fn deserialize_is_compaction<'de, D>(deserializer: D) -> Result<bool, D::Error>
    where
        D: Deserializer<'de>,
    {
        let Some(client_metadata) = Option::<serde_json::Value>::deserialize(deserializer)? else {
            return Ok(false);
        };
        let Some(client_metadata) = client_metadata.as_object() else {
            debug!(
                metadata_type = json_value_type(&client_metadata),
                "GPT client_metadata 不是 JSON 对象，压缩请求分类降级为普通请求"
            );
            return Ok(false);
        };
        let Some(encoded_turn_metadata) = client_metadata.get(CODEX_TURN_METADATA_KEY) else {
            return Ok(false);
        };
        let Some(encoded_turn_metadata) = encoded_turn_metadata.as_str() else {
            debug!(
                metadata_key = CODEX_TURN_METADATA_KEY,
                metadata_value_type = json_value_type(encoded_turn_metadata),
                "GPT Codex turn metadata 不是 JSON 字符串，压缩请求分类降级为普通请求"
            );
            return Ok(false);
        };

        let turn_metadata = match serde_json::from_str::<CodexTurnMetadata>(encoded_turn_metadata) {
            Ok(turn_metadata) => turn_metadata,
            Err(source) => {
                debug!(
                    metadata_key = CODEX_TURN_METADATA_KEY,
                    error = %source,
                    "GPT Codex turn metadata 嵌套 JSON 无效，压缩请求分类降级为普通请求"
                );
                return Ok(false);
            }
        };
        let is_compaction = turn_metadata.request_kind.as_deref() == Some(COMPACTION_REQUEST_KIND);
        debug!(
            metadata_key = CODEX_TURN_METADATA_KEY,
            request_kind_present = turn_metadata.request_kind.is_some(),
            is_compaction,
            "GPT 请求已根据 Codex turn metadata 完成压缩分类"
        );
        Ok(is_compaction)
    }

    fn json_value_type(value: &serde_json::Value) -> &'static str {
        match value {
            serde_json::Value::Null => "null",
            serde_json::Value::Bool(_) => "bool",
            serde_json::Value::Number(_) => "number",
            serde_json::Value::String(_) => "string",
            serde_json::Value::Array(_) => "array",
            serde_json::Value::Object(_) => "object",
        }
    }

    fn deserialize_required_trimmed_string<'de, D>(deserializer: D) -> Result<String, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        let value = value.trim().to_owned();
        if value.is_empty() {
            return Err(de::Error::custom("字段不能为空"));
        }

        Ok(value)
    }

    fn deserialize_optional_trimmed_string<'de, D>(
        deserializer: D,
    ) -> Result<Option<String>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Option::<String>::deserialize(deserializer)?;
        Ok(value
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty()))
    }
}

pub mod header {
    use std::sync::Arc;

    use axum::http::{HeaderMap, HeaderName, HeaderValue, header};
    use reqwest::{
        Url,
        cookie::{CookieStore, Jar},
    };
    use tracing::{debug, warn};

    use crate::{
        err::{AppError, AppResult},
        provider::{
            gpt::model::PROVIDER, request_header::build_transparent_official_api_key_headers,
        },
    };

    const CHATGPT_ACCOUNT_ID_HEADER: &str = "chatgpt-account-id";
    const FEDRAMP_HEADER: &str = "x-openai-fedramp";
    const CODEX_VERSION_HEADER: &str = "version";
    pub const ORIGINATOR_HEADER: &str = "originator";
    pub const FALLBACK_CODEX_CLIENT: &str = "codex_cli_rs";
    pub const FALLBACK_CODEX_USER_AGENT: &str = "codex_cli_rs/0.144.1 (Ubuntu 22.4.0; x86_64)";
    const JSON_CONTENT_TYPE: &str = "application/json";
    const SSE_CONTENT_TYPE: &str = "text/event-stream";
    const SUPPORTED_CODEX_CLIENTS: &[&str] = &[
        "codex_cli_rs",
        "codex-tui",
        "codex_vscode",
        "codex_vscode_copilot",
        "codex_app",
        "codex_chatgpt_desktop",
        "codex_atlas",
        "codex_exec",
        "codex_sdk_ts",
    ];

    pub(crate) fn cloudflare_cookie_store() -> Arc<ChatGptCloudflareCookieStore> {
        Arc::new(ChatGptCloudflareCookieStore::default())
    }

    /// ChatGPT Cloudflare cookie 存储。
    ///
    /// 官方 Codex 客户端会在 reqwest client 上挂一个进程内 cookie jar，但只允许
    /// Cloudflare 基础设施 cookie。这里保持同样的安全边界：拒绝保存 ChatGPT session、
    /// auth token 等任何账号相关 cookie，避免账号池里不同账号串用浏览器态。
    #[derive(Debug, Default)]
    pub(crate) struct ChatGptCloudflareCookieStore {
        jar: Jar,
    }

    impl CookieStore for ChatGptCloudflareCookieStore {
        fn set_cookies(&self, cookie_headers: &mut dyn Iterator<Item = &HeaderValue>, url: &Url) {
            if !is_chatgpt_cookie_url(url) {
                return;
            }

            let mut cloudflare_cookie_headers =
                cookie_headers.filter(|header| is_allowed_cloudflare_set_cookie_header(header));
            self.jar.set_cookies(&mut cloudflare_cookie_headers, url);
        }

        fn cookies(&self, url: &Url) -> Option<HeaderValue> {
            if !is_chatgpt_cookie_url(url) {
                return None;
            }

            self.jar.cookies(url).and_then(only_cloudflare_cookies)
        }
    }

    fn is_chatgpt_cookie_url(url: &Url) -> bool {
        if url.scheme() != "https" {
            return false;
        }

        let Some(host) = url.host_str() else {
            return false;
        };

        is_allowed_chatgpt_host(host)
    }

    fn is_allowed_chatgpt_host(host: &str) -> bool {
        matches!(
            host,
            "chatgpt.com" | "chat.openai.com" | "chatgpt-staging.com"
        ) || host.ends_with(".chatgpt.com")
            || host.ends_with(".chatgpt-staging.com")
    }

    fn is_allowed_cloudflare_set_cookie_header(header: &HeaderValue) -> bool {
        header
            .to_str()
            .ok()
            .and_then(set_cookie_name)
            .is_some_and(is_allowed_cloudflare_cookie_name)
    }

    fn set_cookie_name(header: &str) -> Option<&str> {
        let (name, _) = header.split_once('=')?;
        let name = name.trim();
        (!name.is_empty()).then_some(name)
    }

    fn only_cloudflare_cookies(header: HeaderValue) -> Option<HeaderValue> {
        let header = header.to_str().ok()?;
        let cookies = header
            .split(';')
            .filter_map(|cookie| {
                let cookie = cookie.trim();
                let name = cookie.split_once('=')?.0.trim();
                is_allowed_cloudflare_cookie_name(name).then_some(cookie)
            })
            .collect::<Vec<_>>()
            .join("; ");

        if cookies.is_empty() {
            None
        } else {
            HeaderValue::from_str(&cookies).ok()
        }
    }

    fn is_allowed_cloudflare_cookie_name(name: &str) -> bool {
        matches!(
            name,
            "__cf_bm"
                | "__cflb"
                | "__cfruid"
                | "__cfseq"
                | "__cfwaitingroom"
                | "_cfuvid"
                | "cf_clearance"
                | "cf_ob_info"
                | "cf_use_ob"
        ) || name.starts_with("cf_chl_")
    }

    /// Codex OAuth 账号调用上游时必须由账号池注入的认证上下文。
    #[derive(Debug, Clone, Copy)]
    pub struct CodexAccountAuth<'a> {
        pub access_token: &'a str,
        pub chatgpt_account_id: Option<&'a str>,
        pub chatgpt_account_is_fedramp: bool,
    }

    /// 按 Codex OAuth 客户端语义构造上游请求头。
    ///
    /// 调用方请求头只保留 Codex/OpenAI 协议头；账号认证头、可选 ChatGPT 账号 ID、
    /// JSON Content-Type 和 SSE Accept 由这里统一写入，避免下游入口把调用方的
    /// `Authorization`、`chatgpt-account-id` 等上下文带到真实上游。
    /// 生成 Codex 账号请求的 provider 基础 header。
    ///
    /// 返回后由通用请求流程应用上游资源 override，随后再调用 `apply_codex_credential`
    /// 注入认证，保证管理员 override 无法替换真正的账号凭证。
    pub fn build_codex_upstream_headers(
        source_headers: &HeaderMap,
        fallback_version_header: Option<&str>,
    ) -> HeaderMap {
        let mut upstream_headers = base_upstream_headers(source_headers);
        apply_codex_client_identity(source_headers, &mut upstream_headers);
        let version_header = HeaderName::from_static(CODEX_VERSION_HEADER);
        if !upstream_headers.contains_key(&version_header)
            && let Some(version) = fallback_version_header
                .map(str::trim)
                .filter(|value| !value.is_empty())
        {
            match HeaderValue::from_str(version) {
                Ok(value) => {
                    upstream_headers.insert(version_header, value);
                }
                Err(error) => {
                    warn!(
                        version,
                        error = %error,
                        "AESTUS_CODEX_VERSION_HEADER 不是合法 HTTP header value，跳过 version header 注入"
                    );
                }
            }
        }

        upstream_headers
    }

    /// 为 Codex Images 账号端点构造最小 JSON 请求头。
    ///
    /// Images 不使用 Responses 的 SSE Accept、openai-beta 或 turn header，只保留下游语言
    /// 偏好，并复用同一套受信任 Codex 客户端身份。账号凭证仍在资源 override 之后注入。
    pub fn build_codex_image_upstream_headers(source_headers: &HeaderMap) -> HeaderMap {
        let mut upstream_headers = HeaderMap::new();
        for value in source_headers.get_all(header::ACCEPT_LANGUAGE) {
            upstream_headers.append(header::ACCEPT_LANGUAGE, value.clone());
        }
        apply_codex_client_identity(source_headers, &mut upstream_headers);
        apply_codex_image_protocol_headers(&mut upstream_headers);
        upstream_headers
    }

    /// 在资源 override 之后恢复 Codex Images 的权威协议 header。
    pub fn apply_codex_image_protocol_headers(upstream_headers: &mut HeaderMap) {
        upstream_headers.insert(header::ACCEPT, HeaderValue::from_static("application/json"));
        upstream_headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static(JSON_CONTENT_TYPE),
        );
        // Images 端点不使用 Responses experimental 协议标记；管理员 override 也不能把
        // 它重新注入到最终请求。
        upstream_headers.remove("openai-beta");
    }

    pub fn apply_codex_credential(
        upstream_headers: &mut HeaderMap,
        account_auth: CodexAccountAuth<'_>,
    ) -> AppResult<()> {
        // override 可以包含这些字段，但认证注入必须拥有最终写入权。
        upstream_headers.remove(header::AUTHORIZATION);
        upstream_headers.remove(CHATGPT_ACCOUNT_ID_HEADER);
        upstream_headers.remove(FEDRAMP_HEADER);

        let bearer = format!("Bearer {}", account_auth.access_token);
        let mut value =
            HeaderValue::from_str(&bearer).map_err(|source| AppError::ProviderUpstream {
                provider: PROVIDER.to_owned(),
                message: format!("Codex access token 无法写入 Authorization header: {source}"),
            })?;
        value.set_sensitive(true);
        upstream_headers.insert(header::AUTHORIZATION, value);

        let account_id = account_auth
            .chatgpt_account_id
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if let Some(account_id) = account_id {
            let value =
                HeaderValue::from_str(account_id).map_err(|source| AppError::ProviderUpstream {
                    provider: PROVIDER.to_owned(),
                    message: format!("chatgpt_account_id 无法写入上游 header: {source}"),
                })?;
            upstream_headers.insert(CHATGPT_ACCOUNT_ID_HEADER, value);
        }

        if account_auth.chatgpt_account_is_fedramp {
            upstream_headers.insert(FEDRAMP_HEADER, HeaderValue::from_static("true"));
        }
        Ok(())
    }

    /// 按官方 API Key 的透明代理语义构造上游请求头。
    ///
    /// 除网关认证、连接级和 framing header 外，调用方 header 均原样透传；管理员 override
    /// 随后仍可施加显式配置，最终 hook 只保证真实 OpenAI `Authorization` 拥有写入权。
    pub fn build_official_api_key_upstream_headers(source_headers: &HeaderMap) -> HeaderMap {
        build_transparent_official_api_key_headers(source_headers)
    }

    pub fn apply_official_api_key_credential(
        upstream_headers: &mut HeaderMap,
        api_key: &str,
    ) -> AppResult<()> {
        upstream_headers.remove(header::AUTHORIZATION);
        let bearer = format!("Bearer {api_key}");
        let mut value =
            HeaderValue::from_str(&bearer).map_err(|source| AppError::ProviderUpstream {
                provider: PROVIDER.to_owned(),
                message: format!("官方 API Key 无法写入 Authorization header: {source}"),
            })?;
        value.set_sensitive(true);
        upstream_headers.insert(header::AUTHORIZATION, value);
        Ok(())
    }

    fn base_upstream_headers(source_headers: &HeaderMap) -> HeaderMap {
        let mut upstream_headers = HeaderMap::new();
        for (name, value) in source_headers {
            if should_forward_request_header(name) {
                upstream_headers.append(name.clone(), value.clone());
            }
        }

        upstream_headers.insert(header::ACCEPT, event_stream_content_type());
        upstream_headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static(JSON_CONTENT_TYPE),
        );

        upstream_headers
    }

    /// 根据下游 User-Agent 生成上游 Codex 客户端身份。
    ///
    /// 只以第一个 `/` 之前的片段识别官方 client；命中白名单时完整保留下游 UA，未携带、
    /// 非 UTF-8、缺少 `/` 或 client 不受支持时统一使用固定 fallback。originator 不独立信任
    /// 下游值，而是始终与最终 UA 的 client 同步。资源 override 在本步骤之后应用，仍可按
    /// 管理员配置覆盖这两个普通协议 header。
    fn apply_codex_client_identity(source_headers: &HeaderMap, upstream_headers: &mut HeaderMap) {
        let downstream_user_agent = source_headers.get(header::USER_AGENT);
        let downstream_user_agent_text =
            downstream_user_agent.and_then(|value| value.to_str().ok());
        let downstream_client = downstream_user_agent_text
            .and_then(|user_agent| user_agent.split_once('/').map(|(client, _)| client.trim()));
        let supported_identity = downstream_user_agent
            .zip(downstream_client.and_then(supported_codex_client))
            .map(|(user_agent, client)| (user_agent.clone(), client));

        let (user_agent, client, used_fallback) = supported_identity.map_or_else(
            || {
                (
                    HeaderValue::from_static(FALLBACK_CODEX_USER_AGENT),
                    FALLBACK_CODEX_CLIENT,
                    true,
                )
            },
            |(user_agent, client)| (user_agent, client, false),
        );

        upstream_headers.insert(header::USER_AGENT, user_agent);
        upstream_headers.insert(
            HeaderName::from_static(ORIGINATOR_HEADER),
            HeaderValue::from_static(client),
        );
        debug!(
            downstream_user_agent_present = downstream_user_agent.is_some(),
            downstream_codex_client = downstream_client.unwrap_or("<missing-or-invalid>"),
            resolved_codex_client = client,
            used_fallback,
            "GPT 上游 User-Agent 已校验，originator 已与 client 同步"
        );
    }

    fn supported_codex_client(client: &str) -> Option<&'static str> {
        SUPPORTED_CODEX_CLIENTS
            .iter()
            .copied()
            .find(|supported| *supported == client)
    }

    pub fn should_forward_request_header(name: &HeaderName) -> bool {
        is_codex_passthrough_request_header(name)
            && !is_upstream_rewritten_request_header(name)
            && !is_hop_by_hop_header(name)
    }

    fn is_codex_passthrough_request_header(name: &HeaderName) -> bool {
        let name = name.as_str();
        name == header::ACCEPT_LANGUAGE.as_str()
            || name == CODEX_VERSION_HEADER
            || name == "session-id"
            || name == "thread-id"
            || name == "traceparent"
            || name == "tracestate"
            || name == "openai-beta"
            || name == "openai-organization"
            || name == "openai-project"
            || name == "x-client-request-id"
            || name == "x-codex-installation-id"
            || name == "x-codex-window-id"
            || name == "x-codex-turn-state"
            || name == "x-codex-turn-metadata"
            || name == "x-codex-parent-thread-id"
            || name == "x-oai-attestation"
            || name == "x-openai-memgen-request"
            || name == "x-openai-subagent"
            || name == "x-openai-internal-codex-responses-lite"
            || name == "x-responsesapi-include-timing-metrics"
            || name.starts_with("x-stainless-")
    }

    pub fn is_upstream_rewritten_request_header(name: &HeaderName) -> bool {
        name == header::AUTHORIZATION
            || name == header::ACCEPT
            || name == header::CONTENT_TYPE
            || name == header::USER_AGENT
            || name == header::HOST
            || name == header::CONTENT_LENGTH
            || name == header::COOKIE
            || name == header::SET_COOKIE
            || name.as_str() == CHATGPT_ACCOUNT_ID_HEADER
            || name.as_str() == FEDRAMP_HEADER
            || name.as_str() == ORIGINATOR_HEADER
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

    pub fn event_stream_content_type() -> HeaderValue {
        HeaderValue::from_static(SSE_CONTENT_TYPE)
    }
}

pub mod response {
    use axum::{
        body::Bytes,
        http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header},
    };
    use chrono::{DateTime, Utc};
    use serde::{Deserialize, Serialize};
    use serde_json::Value;

    use crate::provider::protocol::{
        EncodedProviderError, ProviderVisibleError, ProviderVisibleErrorKind,
    };

    // Codex 解析 `response.failed` 时只读取 data JSON 的 `type` 以及
    // `response.error.code/message`。`rate_limit_exceeded` 会进入可重试错误分支，message
    // 中的 28ms 会成为客户端请求的重试延迟；固定字节可以避免再次解析和改写上游 event。
    const CLIENT_RETRY_FAILED_EVENT: &[u8] = br#"event: response.failed
data: {"type":"response.failed","response":{"error":{"code":"rate_limit_exceeded","message":"Please try again in 28ms"}}}

"#;
    const JSON_CONTENT_TYPE: &str = "application/json";
    const X_REQUEST_ID_HEADER: &str = "x-request-id";
    const GPT_ERROR_FALLBACK_BODY: &[u8] = br#"{"error":{"message":"gateway error","type":"server_error","param":null,"code":"gateway_error"}}"#;

    /// GPT/Codex HTTP 失败响应外壳。
    ///
    /// 该结构只编码已经由通用 gateway 脱敏的错误。真实上游返回的失败响应仍然按原始
    /// status/header/body 透传，避免破坏 Codex 客户端对官方错误码和响应头的判断。
    #[derive(Debug, Clone, Serialize, PartialEq, Eq)]
    pub struct GptErrorResponse {
        pub error: GptErrorBody,
    }

    /// OpenAI/Codex 常见 HTTP error wire shape。
    ///
    /// `type` 和 `param` 字段对官方客户端兼容性有意义，因此即使当前 Codex 主要读取
    /// status/body，也保持完整形状，并且不接触带有内部诊断的 `AppError`。
    #[derive(Debug, Clone, Serialize, PartialEq, Eq)]
    pub struct GptErrorBody {
        pub message: String,
        #[serde(rename = "type")]
        pub error_type: &'static str,
        pub param: Option<&'static str>,
        pub code: &'static str,
    }

    pub fn encode_provider_error(
        error: &ProviderVisibleError,
        request_id: uuid::Uuid,
    ) -> EncodedProviderError {
        let error_type = match error.kind {
            ProviderVisibleErrorKind::Authentication => "authentication_error",
            ProviderVisibleErrorKind::Permission | ProviderVisibleErrorKind::InvalidRequest => {
                "invalid_request_error"
            }
            ProviderVisibleErrorKind::RateLimit => "usage_limit_reached",
            ProviderVisibleErrorKind::Gateway => "server_error",
        };
        let payload = GptErrorResponse {
            error: GptErrorBody {
                message: error.message.clone(),
                error_type,
                param: None,
                code: error.code,
            },
        };
        let (status, body) = match serde_json::to_vec(&payload) {
            Ok(body) => (error.status, Bytes::from(body)),
            Err(source) => {
                // 当前 DTO 只含字符串，正常情况下不会失败；仍保留稳定兜底，且兜底正文
                // 与所有内部故障一样只暴露 `gateway error`。
                tracing::error!(
                    request_id = %request_id,
                    error = %source,
                    "GPT provider 可见错误序列化失败，使用 gateway error 固定响应"
                );
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Bytes::from_static(GPT_ERROR_FALLBACK_BODY),
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
                headers.insert(X_REQUEST_ID_HEADER, value);
            }
            Err(source) => {
                tracing::error!(
                    request_id = %request_id,
                    error = %source,
                    "GPT gateway request_id 无法编码为响应头"
                );
            }
        }

        EncodedProviderError {
            status,
            headers,
            body,
        }
    }

    /// 官方 `response.completed.response.usage` 的 token 用量快照。
    ///
    /// 成功响应体仍按字节透传给调用方；该结构只用于旁路提取日志和后续统计需要，
    /// 不参与成功响应的建模式转换。
    #[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
    pub struct CodexTokenUsage {
        pub input_tokens: i64,
        pub cached_input_tokens: i64,
        pub output_tokens: i64,
        pub reasoning_output_tokens: i64,
        pub total_tokens: i64,
    }

    #[derive(Debug, Deserialize)]
    struct ResponseCompletedEvent {
        #[serde(rename = "type")]
        event_type: Option<String>,
        response: Option<ResponseCompletedEnvelope>,
    }

    #[derive(Debug, Deserialize)]
    struct ResponseCompletedEnvelope {
        usage: Option<ResponseCompletedUsage>,
    }

    #[derive(Debug, Deserialize)]
    struct ResponseCompletedUsage {
        input_tokens: i64,
        input_tokens_details: Option<ResponseCompletedInputTokensDetails>,
        output_tokens: i64,
        output_tokens_details: Option<ResponseCompletedOutputTokensDetails>,
        total_tokens: i64,
    }

    #[derive(Debug, Deserialize)]
    struct ResponseCompletedInputTokensDetails {
        cached_tokens: i64,
    }

    #[derive(Debug, Deserialize)]
    struct ResponseCompletedOutputTokensDetails {
        reasoning_tokens: i64,
    }

    impl From<ResponseCompletedUsage> for CodexTokenUsage {
        fn from(value: ResponseCompletedUsage) -> Self {
            Self {
                input_tokens: value.input_tokens,
                cached_input_tokens: value
                    .input_tokens_details
                    .map(|details| details.cached_tokens)
                    .unwrap_or(0),
                output_tokens: value.output_tokens,
                reasoning_output_tokens: value
                    .output_tokens_details
                    .map(|details| details.reasoning_tokens)
                    .unwrap_or(0),
                total_tokens: value.total_tokens,
            }
        }
    }

    pub fn parse_response_completed_usage(body: &[u8]) -> Option<CodexTokenUsage> {
        let event = serde_json::from_slice::<ResponseCompletedEvent>(body).ok()?;
        if event.event_type.as_deref() != Some("response.completed") {
            return None;
        }

        event
            .response
            .and_then(|response| response.usage)
            .map(Into::into)
    }

    /// Codex `response.failed` 事件中的错误对象。
    #[derive(Debug, Clone, Deserialize, PartialEq)]
    pub struct CodexResponseError {
        #[serde(rename = "type")]
        pub error_type: Option<String>,
        pub code: Option<String>,
        pub message: Option<String>,
        pub plan_type: Option<String>,
        pub resets_at: Option<i64>,
    }

    impl CodexResponseError {
        pub fn resets_at_datetime(&self) -> Option<DateTime<Utc>> {
            self.resets_at
                .and_then(|timestamp| DateTime::<Utc>::from_timestamp(timestamp, 0))
        }
    }

    /// Codex/GPT 响应中可被账号池业务消费的账号信号。
    ///
    /// 这里保持协议层语义，不直接依赖 scheduler 的调度结果类型；调用方负责把这些
    /// 信号映射成具体的账号状态变更、冷却、重试等业务动作。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum CodexAccountSignal {
        Unauthorized,
        QuotaExhausted {
            resets_at: Option<DateTime<Utc>>,
        },
        UsageLimitReached {
            plan_type: Option<String>,
            resets_at: Option<DateTime<Utc>>,
        },
        UsageNotIncluded,
    }

    pub fn parse_response_failed_error(body: &[u8]) -> Option<CodexResponseError> {
        let value = serde_json::from_slice::<Value>(body).ok()?;
        if value.get("type").and_then(Value::as_str) != Some("response.failed") {
            return None;
        }

        value
            .get("response")
            .and_then(|response| response.get("error"))
            .and_then(|error| serde_json::from_value(error.clone()).ok())
    }

    /// SSE `data:` 行解析结果。
    #[derive(Debug, Clone, PartialEq)]
    pub enum CodexSseData {
        ResponseFailed(CodexResponseError),
        ResponseCompleted(CodexTokenUsage),
        Other(serde_json::Value),
    }

    pub fn parse_sse_data_json(data: &[u8]) -> Option<CodexSseData> {
        if let Some(error) = parse_response_failed_error(data) {
            return Some(CodexSseData::ResponseFailed(error));
        }

        if let Some(usage) = parse_response_completed_usage(data) {
            return Some(CodexSseData::ResponseCompleted(usage));
        }

        serde_json::from_slice(data).ok().map(CodexSseData::Other)
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct SseEventBoundary {
        pub event_end: usize,
        pub delimiter_len: usize,
    }

    pub fn find_sse_event_boundary(buffer: &[u8]) -> Option<SseEventBoundary> {
        buffer
            .windows(2)
            .position(|window| window == b"\n\n")
            .map(|event_end| SseEventBoundary {
                event_end,
                delimiter_len: 2,
            })
            .or_else(|| {
                buffer
                    .windows(4)
                    .position(|window| window == b"\r\n\r\n")
                    .map(|event_end| SseEventBoundary {
                        event_end,
                        delimiter_len: 4,
                    })
            })
    }

    pub fn collect_sse_event_data(event: &[u8]) -> Option<String> {
        let text = std::str::from_utf8(event).ok()?;
        let mut data = String::new();

        for line in text.lines() {
            let Some(value) = line.strip_prefix("data:") else {
                continue;
            };
            let value = value.trim();
            if value == "[DONE]" {
                return None;
            }
            data.push_str(value);
        }

        (!data.is_empty()).then_some(data)
    }

    pub fn original_sse_event_bytes(event: &[u8], delimiter: &[u8]) -> Bytes {
        let mut bytes = Vec::with_capacity(event.len() + delimiter.len());
        bytes.extend_from_slice(event);
        bytes.extend_from_slice(delimiter);
        Bytes::from(bytes)
    }

    pub fn client_retry_failed_event() -> Bytes {
        Bytes::from_static(CLIENT_RETRY_FAILED_EVENT)
    }

    pub fn should_forward_response_header(name: &HeaderName) -> bool {
        // Set-Cookie 已由 ChatGPT 专用 reqwest cookie provider 在网关内部消费。禁止把
        // Cloudflare 或未来可能出现的账号相关 Cookie 暴露给下游调用方。
        !is_hop_by_hop_header(name)
            && name != header::CONTENT_LENGTH
            && name != header::SET_COOKIE
            && name != header::AUTHORIZATION
            && name.as_str() != "x-api-key"
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

    /// 从 HTTP 失败响应中提取 Codex/GPT 账号信号。
    ///
    /// 普通 4xx/5xx 不在这里做自定义分类，调用方会原样透传给 gateway。
    pub fn parse_account_signal(status: StatusCode, body: &Bytes) -> Option<CodexAccountSignal> {
        if status == StatusCode::UNAUTHORIZED {
            return Some(CodexAccountSignal::Unauthorized);
        }

        let parsed_error = serde_json::from_slice::<Value>(body)
            .ok()
            .and_then(|value| value.get("error").cloned())
            .and_then(|error| serde_json::from_value::<CodexResponseError>(error).ok());

        // Codex 对 HTTP 429 只按 `error.type` 识别 usage 类错误；其他 429 直接交给客户端
        // 按原始 HTTP 错误处理，避免把上游限流或未知 429 误判为账号池调度信号。
        if status == StatusCode::TOO_MANY_REQUESTS {
            return match parsed_error
                .as_ref()
                .and_then(|error| error.error_type.as_deref())
            {
                Some("usage_limit_reached") => Some(CodexAccountSignal::UsageLimitReached {
                    plan_type: parsed_error
                        .as_ref()
                        .and_then(|error| error.plan_type.clone()),
                    resets_at: parsed_error
                        .as_ref()
                        .and_then(CodexResponseError::resets_at_datetime),
                }),
                Some("usage_not_included") => Some(CodexAccountSignal::UsageNotIncluded),
                _ => None,
            };
        }

        None
    }

    pub fn parse_stream_account_signal(error: &CodexResponseError) -> Option<CodexAccountSignal> {
        // 官方 Codex 的 SSE `response.failed` 分类只按 `error.code` 判断这些错误，
        // 不回退读取 `error.type`。这里保持相同语义，避免把 HTTP 429 usage 错误的
        // `type` 字段误用于 SSE 场景，导致账号被错误停用或限额。
        if is_stream_usage_not_included_error(error) {
            return Some(CodexAccountSignal::UsageNotIncluded);
        }

        if is_stream_quota_exceeded_error(error) {
            return Some(CodexAccountSignal::QuotaExhausted {
                resets_at: error.resets_at_datetime(),
            });
        }

        None
    }

    fn is_stream_usage_not_included_error(error: &CodexResponseError) -> bool {
        error.code.as_deref() == Some("usage_not_included")
    }

    fn is_stream_quota_exceeded_error(error: &CodexResponseError) -> bool {
        error.code.as_deref() == Some("insufficient_quota")
    }
}
