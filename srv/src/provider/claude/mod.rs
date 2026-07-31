pub mod auth;
pub mod maintenance;
pub mod messages;
pub mod messages_http;
pub mod model;
pub mod sql;

/// Anthropic OAuth Bearer 凭证在 token 刷新和模型请求上都要求声明的 beta。
pub const OAUTH_BETA: &str = "oauth-2025-04-20";
