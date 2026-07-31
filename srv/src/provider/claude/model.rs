use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const PROVIDER: &str = "claude";

/// Claude Code 当前支持的 Claude.ai 付费订阅类型。
///
/// Profile API 返回的 `organization_type` 带有 `claude_` 前缀；持久化时统一转换成这里
/// 的稳定短名称，避免管理端和后续调度逻辑依赖上游传输层命名。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaudeSubscriptionType {
    Max,
    Pro,
    Team,
    Enterprise,
}

impl ClaudeSubscriptionType {
    pub fn from_organization_type(organization_type: &str) -> Option<Self> {
        match organization_type {
            "claude_max" => Some(Self::Max),
            "claude_pro" => Some(Self::Pro),
            "claude_team" => Some(Self::Team),
            "claude_enterprise" => Some(Self::Enterprise),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Max => "max",
            Self::Pro => "pro",
            Self::Team => "team",
            Self::Enterprise => "enterprise",
        }
    }
}

/// Claude 账号发布到 Redis 的最小请求上下文。
///
/// OAuth access token 虽然已经标识账号，但 Claude Code 仍会在 Messages
/// `metadata.user_id` 的 JSON 字符串中携带 account UUID 用于 OAuth attribution。
/// 其他 Profile、管理展示和 maintenance 字段不参与请求构造，因此不能进入 runtime。
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ClaudeAccountRequestContext {
    pub account_uuid: Uuid,
}

/// Claude OAuth token 与 Profile 响应中可稳定获得的 provider 私有身份信息。
///
/// `organization_uuid` 是授权时 Anthropic 返回的组织标识；OAuth 响应目前没有独立的
/// Workspace 字段，因此这里不把组织 UUID 推断成 workspace ID。后续若上游明确返回
/// workspace，再以新字段扩展该 JSON 结构。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClaudeAccountSpecific {
    #[serde(default)]
    pub account_uuid: Option<String>,
    #[serde(default)]
    pub organization_uuid: Option<String>,
    #[serde(default)]
    pub email_address: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
    /// OAuth Profile 校验后的规范化付费订阅类型。
    #[serde(default)]
    pub subscription_type: Option<ClaudeSubscriptionType>,
    #[serde(default)]
    pub rate_limit_tier: Option<String>,
    #[serde(default)]
    pub has_extra_usage_enabled: Option<bool>,
    #[serde(default)]
    pub billing_type: Option<String>,
    #[serde(default)]
    pub account_created_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub subscription_created_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub scopes: Vec<String>,
    #[serde(default)]
    pub refresh_token_expires_at: Option<DateTime<Utc>>,
}
