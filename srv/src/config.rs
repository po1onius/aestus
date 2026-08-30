use std::{env, net::SocketAddr, num::NonZeroU32};

use chrono_tz::Tz;
use tracing::warn;

use crate::err::{AppError, AppResult};

const DEFAULT_BIND_ADDR: &str = "127.0.0.1:8080";
const DEFAULT_BODY_MEMORY_LIMIT_BYTES: usize = 1024 * 1024;
const DEFAULT_UPSTREAM_RETRY_LIMIT: u8 = 2;
const DEFAULT_DATABASE_POOL_SIZE: usize = 16;
const DEFAULT_PROVIDER_SESSION_STICKY_TTL_SECONDS: u64 = 3600;
const DEFAULT_PROVIDER_SCHEDULER_CANDIDATE_LIMIT: i64 = 32;
const DEFAULT_GPT_UPSTREAM_API_KEY_PROBE_INTERVAL_SECONDS: u64 = 120;
const DEFAULT_GPT_UPSTREAM_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
const DEFAULT_GPT_UPSTREAM_RESPONSES_PATH: &str = "/responses";
const DEFAULT_GPT_UPSTREAM_SEARCH_PATH: &str = "/alpha/search";
const DEFAULT_GPT_UPSTREAM_IMAGE_GENERATIONS_PATH: &str = "/images/generations";
const DEFAULT_GPT_UPSTREAM_IMAGE_EDITS_PATH: &str = "/images/edits";
const DEFAULT_PROVIDER_UPSTREAM_CONNECT_TIMEOUT_SECONDS: u64 = 30;
const DEFAULT_PROVIDER_UPSTREAM_TIMEOUT_SECONDS: u64 = 120;
// SSE 默认允许无限时长且不启用空闲超时；部署方显式配置正数后，只有连续无上游字节
// 达到该秒数才终止流，活跃的长响应不会受总时长限制。
const DEFAULT_PROVIDER_UPSTREAM_STREAM_IDLE_TIMEOUT_SECONDS: u64 = 0;
const DEFAULT_GPT_TOKEN_REFRESH_AHEAD_SECONDS: u64 = 180;
const DEFAULT_GPT_TOKEN_REFRESH_RETRY_SECONDS: u64 = 30;
const DEFAULT_GPT_OAUTH_ISSUER: &str = "https://auth.openai.com";
const DEFAULT_GPT_OAUTH_REDIRECT_URI: &str = "http://localhost:1455/auth/callback";
const DEFAULT_GPT_OAUTH_SCOPE: &str = "openid profile email offline_access";
const DEFAULT_GPT_OAUTH_SESSION_TTL_SECONDS: u64 = 600;
const DEFAULT_GPT_TOKEN_ENDPOINT: &str = "https://auth.openai.com/oauth/token";
const DEFAULT_GPT_QUOTA_RECOVERY_SECONDS: u64 = 5 * 60 * 60;
const DEFAULT_CLAUDE_ACCOUNT_RATE_LIMIT_COOLDOWN_SECONDS: u64 = 120;
const DEFAULT_CLAUDE_UPSTREAM_API_KEY_PROBE_INTERVAL_SECONDS: u64 = 120;
const DEFAULT_CLAUDE_UPSTREAM_API_KEY_PROBE_MODEL: &str = "claude-opus-4-8";
const DEFAULT_CLAUDE_UPSTREAM_BASE_URL: &str = "https://api.anthropic.com";
const DEFAULT_CLAUDE_UPSTREAM_MESSAGES_PATH: &str = "/v1/messages";
const DEFAULT_CLAUDE_TOKEN_REFRESH_AHEAD_SECONDS: u64 = 180;
const DEFAULT_CLAUDE_TOKEN_REFRESH_RETRY_SECONDS: u64 = 30;
const DEFAULT_CLAUDE_OAUTH_AUTHORIZE_URL: &str = "https://claude.com/cai/oauth/authorize";
const DEFAULT_CLAUDE_OAUTH_REDIRECT_URI: &str = "https://platform.claude.com/oauth/code/callback";
const DEFAULT_CLAUDE_OAUTH_SCOPE: &str =
    "user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload";
const DEFAULT_CLAUDE_OAUTH_SESSION_TTL_SECONDS: u64 = 600;
const DEFAULT_CLAUDE_TOKEN_ENDPOINT: &str = "https://platform.claude.com/v1/oauth/token";
const DEFAULT_CLICKHOUSE_URL: &str = "http://localhost:8123";
const DEFAULT_CLICKHOUSE_DATABASE: &str = "default";
const DEFAULT_CLICKHOUSE_USER: &str = "default";
const DEFAULT_REQUEST_LOG_TABLE: &str = "gateway_request_logs";
const DEFAULT_REQUEST_USAGE_DAILY_TABLE: &str = "gateway_request_usage_daily";
const DEFAULT_REQUEST_LOG_RETENTION_DAYS: NonZeroU32 = NonZeroU32::new(30).unwrap();
const DEFAULT_SERVICE_TIMEZONE: Tz = chrono_tz::UTC;
const DEFAULT_JWT_TTL_SECONDS: u64 = 7 * 24 * 60 * 60;
const DEFAULT_ADMIN_INITIAL_QUOTA: i64 = 1_000_000;
const DEFAULT_SMTP_PORT: u16 = 587;
const DEFAULT_EMAIL_CODE_TTL_SECONDS: u64 = 600;
const DEFAULT_EMAIL_CODE_COOLDOWN_SECONDS: u64 = 60;

/// 应用配置。
///
/// 配置统一从环境变量读取，后续部署到容器或 systemd 时不需要改代码。
/// 如果出现缺少数据库、Redis 等运行环境的问题，应在启动文档中说明环境修正方式，
/// 不在业务代码中加入环境兼容分支。
#[derive(Debug, Clone)]
pub struct AppConfig {
    pub bind_addr: SocketAddr,
    pub database_url: String,
    pub database_pool_size: usize,
    pub redis_url: String,
    pub body_memory_limit_bytes: usize,
    pub upstream_retry_limit: u8,
    pub provider_session_sticky_ttl_seconds: u64,
    pub provider_scheduler_candidate_limit: i64,
    pub gpt_upstream_api_key_probe_interval_seconds: u64,
    pub gpt_upstream_base_url: String,
    pub gpt_upstream_responses_path: String,
    pub gpt_upstream_search_path: String,
    pub gpt_upstream_image_generations_path: String,
    pub gpt_upstream_image_edits_path: String,
    pub provider_upstream_connect_timeout_seconds: u64,
    pub provider_upstream_timeout_seconds: u64,
    pub provider_upstream_stream_idle_timeout_seconds: u64,
    pub gpt_token_refresh_ahead_seconds: u64,
    pub gpt_token_refresh_retry_seconds: u64,
    pub gpt_oauth_issuer: String,
    pub gpt_oauth_redirect_uri: String,
    pub gpt_oauth_scope: String,
    pub gpt_oauth_session_ttl_seconds: u64,
    pub gpt_token_endpoint: String,
    pub gpt_quota_recovery_seconds: u64,
    pub claude_account_rate_limit_cooldown_seconds: u64,
    pub claude_upstream_api_key_probe_interval_seconds: u64,
    pub claude_upstream_api_key_probe_model: String,
    pub claude_upstream_base_url: String,
    pub claude_upstream_messages_path: String,
    pub claude_token_refresh_ahead_seconds: u64,
    pub claude_token_refresh_retry_seconds: u64,
    pub claude_oauth_authorize_url: String,
    pub claude_oauth_redirect_uri: String,
    pub claude_oauth_scope: String,
    pub claude_oauth_session_ttl_seconds: u64,
    pub claude_token_endpoint: String,
    pub codex_version_header: Option<String>,
    pub clickhouse_url: String,
    pub clickhouse_database: String,
    pub clickhouse_user: String,
    pub clickhouse_password: String,
    pub request_log_table: String,
    pub request_usage_daily_table: String,
    /// ClickHouse 请求级明细的滚动保留天数；服务启动时会把该值同步到表 TTL。
    pub request_log_retention_days: NonZeroU32,
    /// 请求日志日期、每日用量统计和 Dashboard 日期选择共用的固定 IANA 时区。
    /// 修改该值会改变业务日边界；已经聚合的数据不能在明细 TTL 到期后自动重算。
    pub service_timezone: Tz,
    pub web_dist_dir: Option<String>,
    pub jwt_secret: String,
    pub jwt_ttl_seconds: u64,
    pub admin_username: String,
    pub admin_email: String,
    pub admin_password: String,
    pub admin_initial_quota: i64,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_username: String,
    pub smtp_password: String,
    pub smtp_from: String,
    pub email_code_ttl_seconds: u64,
    pub email_code_cooldown_seconds: u64,
}

impl AppConfig {
    pub fn from_env() -> AppResult<Self> {
        Ok(Self {
            bind_addr: parse_env("AESTUS_BIND_ADDR", default_bind_addr())?,
            database_url: required_env("DATABASE_URL")?,
            database_pool_size: parse_env("AESTUS_DATABASE_POOL_SIZE", DEFAULT_DATABASE_POOL_SIZE)?,
            redis_url: required_env("REDIS_URL")?,
            body_memory_limit_bytes: parse_env(
                "AESTUS_BODY_MEMORY_LIMIT_BYTES",
                DEFAULT_BODY_MEMORY_LIMIT_BYTES,
            )?,
            upstream_retry_limit: parse_env(
                "AESTUS_UPSTREAM_RETRY_LIMIT",
                DEFAULT_UPSTREAM_RETRY_LIMIT,
            )?,
            provider_session_sticky_ttl_seconds: parse_env(
                "AESTUS_PROVIDER_SESSION_STICKY_TTL_SECONDS",
                DEFAULT_PROVIDER_SESSION_STICKY_TTL_SECONDS,
            )?,
            provider_scheduler_candidate_limit: parse_env(
                "AESTUS_PROVIDER_SCHEDULER_CANDIDATE_LIMIT",
                DEFAULT_PROVIDER_SCHEDULER_CANDIDATE_LIMIT,
            )?,
            gpt_upstream_api_key_probe_interval_seconds: parse_env(
                "AESTUS_GPT_UPSTREAM_API_KEY_PROBE_INTERVAL_SECONDS",
                DEFAULT_GPT_UPSTREAM_API_KEY_PROBE_INTERVAL_SECONDS,
            )?,
            gpt_upstream_base_url: parse_env_string(
                "AESTUS_GPT_UPSTREAM_BASE_URL",
                DEFAULT_GPT_UPSTREAM_BASE_URL,
            )?,
            gpt_upstream_responses_path: parse_env_string(
                "AESTUS_GPT_UPSTREAM_RESPONSES_PATH",
                DEFAULT_GPT_UPSTREAM_RESPONSES_PATH,
            )?,
            gpt_upstream_search_path: parse_env_string(
                "AESTUS_GPT_UPSTREAM_SEARCH_PATH",
                DEFAULT_GPT_UPSTREAM_SEARCH_PATH,
            )?,
            gpt_upstream_image_generations_path: parse_env_string(
                "AESTUS_GPT_UPSTREAM_IMAGE_GENERATIONS_PATH",
                DEFAULT_GPT_UPSTREAM_IMAGE_GENERATIONS_PATH,
            )?,
            gpt_upstream_image_edits_path: parse_env_string(
                "AESTUS_GPT_UPSTREAM_IMAGE_EDITS_PATH",
                DEFAULT_GPT_UPSTREAM_IMAGE_EDITS_PATH,
            )?,
            provider_upstream_connect_timeout_seconds: parse_env(
                "AESTUS_PROVIDER_UPSTREAM_CONNECT_TIMEOUT_SECONDS",
                DEFAULT_PROVIDER_UPSTREAM_CONNECT_TIMEOUT_SECONDS,
            )?,
            provider_upstream_timeout_seconds: parse_env(
                "AESTUS_PROVIDER_UPSTREAM_TIMEOUT_SECONDS",
                DEFAULT_PROVIDER_UPSTREAM_TIMEOUT_SECONDS,
            )?,
            provider_upstream_stream_idle_timeout_seconds: parse_env(
                "AESTUS_PROVIDER_UPSTREAM_STREAM_IDLE_TIMEOUT_SECONDS",
                DEFAULT_PROVIDER_UPSTREAM_STREAM_IDLE_TIMEOUT_SECONDS,
            )?,
            gpt_token_refresh_ahead_seconds: parse_env(
                "AESTUS_GPT_TOKEN_REFRESH_AHEAD_SECONDS",
                DEFAULT_GPT_TOKEN_REFRESH_AHEAD_SECONDS,
            )?,
            gpt_token_refresh_retry_seconds: parse_env(
                "AESTUS_GPT_TOKEN_REFRESH_RETRY_SECONDS",
                DEFAULT_GPT_TOKEN_REFRESH_RETRY_SECONDS,
            )?,
            gpt_oauth_issuer: parse_env_string(
                "AESTUS_GPT_OAUTH_ISSUER",
                DEFAULT_GPT_OAUTH_ISSUER,
            )?,
            gpt_oauth_redirect_uri: parse_env_string(
                "AESTUS_GPT_OAUTH_REDIRECT_URI",
                DEFAULT_GPT_OAUTH_REDIRECT_URI,
            )?,
            gpt_oauth_scope: parse_env_string("AESTUS_GPT_OAUTH_SCOPE", DEFAULT_GPT_OAUTH_SCOPE)?,
            gpt_oauth_session_ttl_seconds: parse_env(
                "AESTUS_GPT_OAUTH_SESSION_TTL_SECONDS",
                DEFAULT_GPT_OAUTH_SESSION_TTL_SECONDS,
            )?,
            gpt_token_endpoint: parse_env_string(
                "AESTUS_GPT_TOKEN_ENDPOINT",
                DEFAULT_GPT_TOKEN_ENDPOINT,
            )?,
            gpt_quota_recovery_seconds: parse_env(
                "AESTUS_GPT_QUOTA_RECOVERY_SECONDS",
                DEFAULT_GPT_QUOTA_RECOVERY_SECONDS,
            )?,
            claude_account_rate_limit_cooldown_seconds: parse_env(
                "AESTUS_CLAUDE_ACCOUNT_RATE_LIMIT_COOLDOWN_SECONDS",
                DEFAULT_CLAUDE_ACCOUNT_RATE_LIMIT_COOLDOWN_SECONDS,
            )?,
            claude_upstream_api_key_probe_interval_seconds: parse_env(
                "AESTUS_CLAUDE_UPSTREAM_API_KEY_PROBE_INTERVAL_SECONDS",
                DEFAULT_CLAUDE_UPSTREAM_API_KEY_PROBE_INTERVAL_SECONDS,
            )?,
            claude_upstream_api_key_probe_model: parse_env_string(
                "AESTUS_CLAUDE_UPSTREAM_API_KEY_PROBE_MODEL",
                DEFAULT_CLAUDE_UPSTREAM_API_KEY_PROBE_MODEL,
            )?,
            claude_upstream_base_url: parse_env_string(
                "AESTUS_CLAUDE_UPSTREAM_BASE_URL",
                DEFAULT_CLAUDE_UPSTREAM_BASE_URL,
            )?,
            claude_upstream_messages_path: parse_env_string(
                "AESTUS_CLAUDE_UPSTREAM_MESSAGES_PATH",
                DEFAULT_CLAUDE_UPSTREAM_MESSAGES_PATH,
            )?,
            claude_token_refresh_ahead_seconds: parse_env(
                "AESTUS_CLAUDE_TOKEN_REFRESH_AHEAD_SECONDS",
                DEFAULT_CLAUDE_TOKEN_REFRESH_AHEAD_SECONDS,
            )?,
            claude_token_refresh_retry_seconds: parse_env(
                "AESTUS_CLAUDE_TOKEN_REFRESH_RETRY_SECONDS",
                DEFAULT_CLAUDE_TOKEN_REFRESH_RETRY_SECONDS,
            )?,
            claude_oauth_authorize_url: parse_env_string(
                "AESTUS_CLAUDE_OAUTH_AUTHORIZE_URL",
                DEFAULT_CLAUDE_OAUTH_AUTHORIZE_URL,
            )?,
            claude_oauth_redirect_uri: parse_env_string(
                "AESTUS_CLAUDE_OAUTH_REDIRECT_URI",
                DEFAULT_CLAUDE_OAUTH_REDIRECT_URI,
            )?,
            claude_oauth_scope: parse_env_string(
                "AESTUS_CLAUDE_OAUTH_SCOPE",
                DEFAULT_CLAUDE_OAUTH_SCOPE,
            )?,
            claude_oauth_session_ttl_seconds: parse_env(
                "AESTUS_CLAUDE_OAUTH_SESSION_TTL_SECONDS",
                DEFAULT_CLAUDE_OAUTH_SESSION_TTL_SECONDS,
            )?,
            claude_token_endpoint: parse_env_string(
                "AESTUS_CLAUDE_TOKEN_ENDPOINT",
                DEFAULT_CLAUDE_TOKEN_ENDPOINT,
            )?,
            codex_version_header: optional_env_string("AESTUS_CODEX_VERSION_HEADER")?,
            clickhouse_url: parse_env_string("CLICKHOUSE_URL", DEFAULT_CLICKHOUSE_URL)?,
            clickhouse_database: parse_env_string(
                "CLICKHOUSE_DATABASE",
                DEFAULT_CLICKHOUSE_DATABASE,
            )?,
            clickhouse_user: parse_env_string("CLICKHOUSE_USER", DEFAULT_CLICKHOUSE_USER)?,
            // ClickHouse 保存请求日志，是管理端日志页的必要依赖。
            // 密码缺失时必须在启动阶段直接失败，避免服务运行后才在查询日志时反复暴露认证错误。
            clickhouse_password: required_env("CLICKHOUSE_PASSWORD")?,
            request_log_table: parse_env_string(
                "AESTUS_REQUEST_LOG_TABLE",
                DEFAULT_REQUEST_LOG_TABLE,
            )?,
            request_usage_daily_table: parse_env_string(
                "AESTUS_REQUEST_USAGE_DAILY_TABLE",
                DEFAULT_REQUEST_USAGE_DAILY_TABLE,
            )?,
            request_log_retention_days: parse_env(
                "AESTUS_REQUEST_LOG_RETENTION_DAYS",
                DEFAULT_REQUEST_LOG_RETENTION_DAYS,
            )?,
            service_timezone: parse_env("AESTUS_TIMEZONE", DEFAULT_SERVICE_TIMEZONE)?,
            web_dist_dir: optional_env_string("AESTUS_WEB_DIST_DIR")?,
            jwt_secret: required_env("AESTUS_JWT_SECRET")?,
            jwt_ttl_seconds: parse_env("AESTUS_JWT_TTL_SECONDS", DEFAULT_JWT_TTL_SECONDS)?,
            admin_username: required_env("AESTUS_ADMIN_USERNAME")?,
            admin_email: required_env("AESTUS_ADMIN_EMAIL")?,
            admin_password: required_env("AESTUS_ADMIN_PASSWORD")?,
            admin_initial_quota: parse_env(
                "AESTUS_ADMIN_INITIAL_QUOTA",
                DEFAULT_ADMIN_INITIAL_QUOTA,
            )?,
            smtp_host: required_env("AESTUS_SMTP_HOST")?,
            smtp_port: parse_env("AESTUS_SMTP_PORT", DEFAULT_SMTP_PORT)?,
            smtp_username: required_env("AESTUS_SMTP_USERNAME")?,
            smtp_password: required_env("AESTUS_SMTP_PASSWORD")?,
            smtp_from: required_env("AESTUS_SMTP_FROM")?,
            email_code_ttl_seconds: parse_env(
                "AESTUS_EMAIL_CODE_TTL_SECONDS",
                DEFAULT_EMAIL_CODE_TTL_SECONDS,
            )?,
            email_code_cooldown_seconds: parse_env(
                "AESTUS_EMAIL_CODE_COOLDOWN_SECONDS",
                DEFAULT_EMAIL_CODE_COOLDOWN_SECONDS,
            )?,
        })
    }
}

fn required_env(key: &'static str) -> AppResult<String> {
    match env::var(key) {
        Ok(value) if !value.trim().is_empty() => Ok(value),
        Ok(_) | Err(env::VarError::NotPresent) => Err(AppError::MissingConfig { key }),
        Err(source) => Err(AppError::ReadConfig { key, source }),
    }
}

fn default_bind_addr() -> SocketAddr {
    DEFAULT_BIND_ADDR
        .parse()
        .expect("默认监听地址必须是合法 SocketAddr")
}

fn parse_env_string(key: &'static str, default: &'static str) -> AppResult<String> {
    match env::var(key) {
        Ok(raw) => Ok(raw),
        Err(env::VarError::NotPresent) => {
            warn!(config_key = key, "环境变量未设置，使用默认配置");
            Ok(default.to_owned())
        }
        Err(source) => Err(AppError::ReadConfig { key, source }),
    }
}

fn optional_env_string(key: &'static str) -> AppResult<Option<String>> {
    match env::var(key) {
        Ok(raw) => Ok(Some(raw).filter(|value| !value.trim().is_empty())),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(source) => Err(AppError::ReadConfig { key, source }),
    }
}

fn parse_env<T>(key: &'static str, default: T) -> AppResult<T>
where
    T: std::str::FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    match env::var(key) {
        Ok(raw) => raw.parse::<T>().map_err(|source| AppError::InvalidConfig {
            key,
            value: raw,
            source: Box::new(source),
        }),
        Err(env::VarError::NotPresent) => {
            warn!(config_key = key, "环境变量未设置，使用默认配置");
            Ok(default)
        }
        Err(source) => Err(AppError::ReadConfig { key, source }),
    }
}
