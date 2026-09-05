use chrono::{DateTime, Utc};
use reqwest::{
    Method, StatusCode,
    header::{ACCEPT, HeaderValue},
};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};
use uuid::Uuid;

use crate::{
    err::{AppError, AppResult},
    provider::{
        credential::ProviderAccount,
        gpt::{
            account_api::{GptAccountApiAuth, account_api_url},
            model::{GptAccountSpecific, PROVIDER},
        },
        response_logging::response_body_for_tracing,
    },
    state::AppState,
};

pub(crate) const DEFAULT_LIMIT_ID: &str = "codex";

/// 单个 GPT 账号的额度快照响应。
///
/// 该结构主要服务管理端即时查看，本身不落库。账号列表不会携带该字段，前端只有在用户
/// 主动点击“刷新额度”时才会调用接口并缓存到页面内存态。若主 Codex 额度确认恢复，HTTP
/// 编排层会据此清理既有额度限制，并通过 `quota_limit_removed` 告知前端刷新账号运行态。
#[derive(Debug, Clone, Serialize)]
pub struct GptAccountQuotaResponse {
    pub account_id: Uuid,
    pub chatgpt_account_id: Option<String>,
    pub plan_type: String,
    pub fetched_at: DateTime<Utc>,
    pub primary: Option<GptQuotaSnapshot>,
    pub snapshots: Vec<GptQuotaSnapshot>,
    pub rate_limit_reset_credits: Option<RateLimitResetCreditsSummary>,
    pub quota_limit_removed: bool,
}

impl GptAccountQuotaResponse {
    /// 返回主 Codex 额度所有约束窗口中的最小剩余百分比。
    ///
    /// ChatGPT 可能同时返回短周期 primary 与长周期 secondary 窗口；任一窗口耗尽都会
    /// 阻止真实请求，因此不能看到其中一个窗口大于 0 就提前恢复调度。上游显式声明
    /// `allowed=false` 或 `limit_reached=true` 时同样以协议状态为准。返回 `Some` 即表示
    /// 已获得“当前额度严格大于 0”的充分证据，可用于解除先前的额度限制。
    pub(crate) fn available_remaining_percent(&self) -> Option<f64> {
        let snapshot = self.primary.as_ref().or_else(|| {
            self.snapshots
                .iter()
                .find(|snapshot| snapshot.limit_id == DEFAULT_LIMIT_ID)
                .or_else(|| self.snapshots.first())
        })?;
        if snapshot.allowed == Some(false) || snapshot.limit_reached == Some(true) {
            return None;
        }

        let minimum_remaining = snapshot
            .primary
            .iter()
            .chain(snapshot.secondary.iter())
            .map(|window| window.remaining_percent)
            .reduce(f64::min)?;
        (minimum_remaining > 0.0).then_some(minimum_remaining)
    }
}

/// Codex usage 接口返回的一组额度窗口。
///
/// `rate_limit` 是主 Codex 额度，`additional_rate_limits` 会映射为额外 snapshot。
/// 前端展示时优先使用 `limit_id == "codex"` 的主额度。
#[derive(Debug, Clone, Serialize)]
pub struct GptQuotaSnapshot {
    pub limit_id: String,
    pub limit_name: Option<String>,
    pub allowed: Option<bool>,
    pub limit_reached: Option<bool>,
    pub primary: Option<GptQuotaWindow>,
    pub secondary: Option<GptQuotaWindow>,
    pub credits: Option<GptCreditsSnapshot>,
    pub individual_limit: Option<GptSpendControlLimitSnapshot>,
    pub plan_type: String,
    pub rate_limit_reached_type: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GptQuotaWindow {
    pub used_percent: f64,
    pub remaining_percent: f64,
    pub window_minutes: Option<i64>,
    /// 用上游原始秒数反推的窗口起点，不从取整后的展示分钟数反推。
    pub starts_at: Option<DateTime<Utc>>,
    pub resets_at: Option<DateTime<Utc>>,
    pub reset_after_seconds: Option<i64>,
    /// Dashboard 按账号和窗口汇总的本网关已记录 token；字符串避免前端整数精度丢失。
    /// 未统计或窗口无法完整查询时为 None，成功查询但没有用量时为 "0"。
    pub gateway_total_tokens: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GptCreditsSnapshot {
    pub has_credits: bool,
    pub unlimited: bool,
    pub balance: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GptSpendControlLimitSnapshot {
    pub limit: String,
    pub used: String,
    pub remaining: String,
    pub used_percent: i32,
    pub remaining_percent: i32,
    pub resets_at: Option<DateTime<Utc>>,
    pub reset_after_seconds: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RateLimitResetCreditsSummary {
    pub available_count: i64,
}

#[derive(Debug, Deserialize)]
struct GptUsagePayload {
    plan_type: String,
    #[serde(default)]
    rate_limit: Option<RateLimitStatusDetails>,
    #[serde(default)]
    credits: Option<CreditStatusDetails>,
    #[serde(default)]
    spend_control: Option<SpendControlStatusDetails>,
    #[serde(default)]
    additional_rate_limits: Option<Vec<AdditionalRateLimitDetails>>,
    #[serde(default)]
    rate_limit_reached_type: Option<RateLimitReachedType>,
    #[serde(default)]
    rate_limit_reset_credits: Option<RateLimitResetCreditsSummary>,
}

#[derive(Debug, Deserialize)]
struct RateLimitStatusDetails {
    #[serde(default)]
    allowed: Option<bool>,
    #[serde(default)]
    limit_reached: Option<bool>,
    #[serde(default)]
    primary_window: Option<RateLimitWindowSnapshot>,
    #[serde(default)]
    secondary_window: Option<RateLimitWindowSnapshot>,
}

#[derive(Debug, Deserialize)]
struct RateLimitWindowSnapshot {
    used_percent: f64,
    #[serde(default)]
    limit_window_seconds: Option<i64>,
    #[serde(default)]
    reset_after_seconds: Option<i64>,
    #[serde(default)]
    reset_at: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct CreditStatusDetails {
    #[serde(default)]
    has_credits: bool,
    #[serde(default)]
    unlimited: bool,
    #[serde(default)]
    balance: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SpendControlStatusDetails {
    #[serde(default)]
    individual_limit: Option<SpendControlLimitDetails>,
}

#[derive(Debug, Deserialize)]
struct SpendControlLimitDetails {
    limit: String,
    used: String,
    remaining: String,
    used_percent: i32,
    remaining_percent: i32,
    #[serde(default)]
    reset_after_seconds: Option<i64>,
    #[serde(default)]
    reset_at: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct AdditionalRateLimitDetails {
    limit_name: String,
    metered_feature: String,
    #[serde(default)]
    rate_limit: Option<RateLimitStatusDetails>,
}

#[derive(Debug, Deserialize)]
struct RateLimitReachedType {
    #[serde(rename = "type")]
    kind: String,
}

/// 查询指定账号在 ChatGPT/Codex 后端上的 usage 额度。
pub async fn fetch_account_quota(
    state: &AppState,
    account: &ProviderAccount,
) -> AppResult<GptAccountQuotaResponse> {
    let specific = account.parse_specific::<GptAccountSpecific>()?;
    let auth = GptAccountApiAuth::from_account(account, "查询额度")?;
    let chatgpt_account_id = auth.chatgpt_account_id();

    let url = usage_status_url(&state.config().gpt_upstream_base_url);
    info!(
        gpt_account_id = %account.id,
        chatgpt_account_id,
        usage_url = %url,
        fedramp = auth.is_fedramp(),
        "开始刷新 GPT 账号额度快照"
    );

    let request = auth
        .request(state, Method::GET, &url)
        .header(ACCEPT, HeaderValue::from_static("application/json"));

    let response = request.send().await.map_err(|source| {
        warn!(
            gpt_account_id = %account.id,
            chatgpt_account_id,
            usage_url = %url,
            error = %source,
            "请求 GPT 账号额度接口失败"
        );
        AppError::ProviderUpstream {
            provider: PROVIDER.to_owned(),
            message: format!("请求 GPT 账号额度接口失败: {source}"),
        }
    })?;

    let status = response.status();
    let body = response.bytes().await.map_err(|source| {
        warn!(
            gpt_account_id = %account.id,
            chatgpt_account_id,
            usage_url = %url,
            error = %source,
            "读取 GPT 账号额度响应体失败"
        );
        AppError::ProviderUpstream {
            provider: PROVIDER.to_owned(),
            message: format!("读取 GPT 账号额度响应体失败: {source}"),
        }
    })?;
    if !status.is_success() {
        return Err(map_usage_status_error(
            account,
            chatgpt_account_id,
            &url,
            status,
            &body,
        ));
    }

    let payload = serde_json::from_slice::<GptUsagePayload>(&body).map_err(|source| {
        let tracing_body = response_body_for_tracing(&body);
        warn!(
            gpt_account_id = %account.id,
            chatgpt_account_id,
            usage_url = %url,
            error = %source,
            response_bytes = body.len(),
            upstream_response_body_encoding = tracing_body.encoding(),
            upstream_response_body = %tracing_body.content(),
            "GPT 账号额度响应 JSON 解析失败，完整响应正文已写入 tracing"
        );
        AppError::ProviderUpstream {
            provider: PROVIDER.to_owned(),
            message: format!("GPT 账号额度响应格式无效: {source}"),
        }
    })?;
    let quota = quota_response_from_payload(account, &specific, payload);

    info!(
        gpt_account_id = %account.id,
        chatgpt_account_id,
        plan_type = %quota.plan_type,
        snapshot_count = quota.snapshots.len(),
        has_reset_credits = quota.rate_limit_reset_credits.is_some(),
        "GPT 账号额度快照刷新完成"
    );

    Ok(quota)
}

fn usage_status_url(upstream_base_url: &str) -> String {
    account_api_url(upstream_base_url, "usage", "usage")
}

fn quota_response_from_payload(
    account: &ProviderAccount,
    specific: &GptAccountSpecific,
    payload: GptUsagePayload,
) -> GptAccountQuotaResponse {
    let GptUsagePayload {
        plan_type,
        rate_limit,
        credits,
        spend_control,
        additional_rate_limits,
        rate_limit_reached_type,
        rate_limit_reset_credits,
    } = payload;
    let rate_limit_reached_type = rate_limit_reached_type.map(|value| value.kind);
    let individual_limit = spend_control
        .and_then(|details| details.individual_limit)
        .map(map_individual_limit);
    let mut snapshots = vec![make_quota_snapshot(
        DEFAULT_LIMIT_ID.to_owned(),
        None,
        rate_limit,
        credits.map(map_credits),
        individual_limit,
        plan_type.clone(),
        rate_limit_reached_type,
    )];

    if let Some(additional) = additional_rate_limits {
        snapshots.extend(additional.into_iter().map(|details| {
            make_quota_snapshot(
                details.metered_feature,
                Some(details.limit_name),
                details.rate_limit,
                None,
                None,
                plan_type.clone(),
                None,
            )
        }));
    }

    let primary = snapshots
        .iter()
        .find(|snapshot| snapshot.limit_id == DEFAULT_LIMIT_ID)
        .or_else(|| snapshots.first())
        .cloned();

    GptAccountQuotaResponse {
        account_id: account.id,
        chatgpt_account_id: specific.chatgpt_account_id.clone(),
        plan_type,
        fetched_at: Utc::now(),
        primary,
        snapshots,
        rate_limit_reset_credits,
        quota_limit_removed: false,
    }
}

fn make_quota_snapshot(
    limit_id: String,
    limit_name: Option<String>,
    rate_limit: Option<RateLimitStatusDetails>,
    credits: Option<GptCreditsSnapshot>,
    individual_limit: Option<GptSpendControlLimitSnapshot>,
    plan_type: String,
    rate_limit_reached_type: Option<String>,
) -> GptQuotaSnapshot {
    let (allowed, limit_reached, primary, secondary) = match rate_limit {
        Some(details) => (
            details.allowed,
            details.limit_reached,
            details.primary_window.map(map_rate_limit_window),
            details.secondary_window.map(map_rate_limit_window),
        ),
        None => (None, None, None, None),
    };

    GptQuotaSnapshot {
        limit_id: normalize_limit_id(limit_id),
        limit_name,
        allowed,
        limit_reached,
        primary,
        secondary,
        credits,
        individual_limit,
        plan_type,
        rate_limit_reached_type,
    }
}

fn map_rate_limit_window(window: RateLimitWindowSnapshot) -> GptQuotaWindow {
    let used_percent = window.used_percent.clamp(0.0, 100.0);
    let resets_at = timestamp_seconds_to_datetime(window.reset_at);
    let starts_at = window
        .limit_window_seconds
        .filter(|seconds| *seconds > 0)
        .and_then(chrono::Duration::try_seconds)
        .and_then(|duration| resets_at?.checked_sub_signed(duration));
    GptQuotaWindow {
        used_percent,
        remaining_percent: (100.0 - used_percent).clamp(0.0, 100.0),
        window_minutes: window
            .limit_window_seconds
            .and_then(window_minutes_from_seconds),
        starts_at,
        resets_at,
        reset_after_seconds: window.reset_after_seconds,
        gateway_total_tokens: None,
    }
}

fn map_credits(details: CreditStatusDetails) -> GptCreditsSnapshot {
    GptCreditsSnapshot {
        has_credits: details.has_credits,
        unlimited: details.unlimited,
        balance: details.balance,
    }
}

fn map_individual_limit(details: SpendControlLimitDetails) -> GptSpendControlLimitSnapshot {
    GptSpendControlLimitSnapshot {
        limit: details.limit,
        used: details.used,
        remaining: details.remaining,
        used_percent: details.used_percent,
        remaining_percent: details.remaining_percent,
        resets_at: timestamp_seconds_to_datetime(details.reset_at),
        reset_after_seconds: details.reset_after_seconds,
    }
}

fn window_minutes_from_seconds(seconds: i64) -> Option<i64> {
    if seconds <= 0 {
        return None;
    }

    Some(seconds / 60 + i64::from(seconds % 60 != 0))
}

fn timestamp_seconds_to_datetime(timestamp: Option<i64>) -> Option<DateTime<Utc>> {
    timestamp.and_then(|timestamp| DateTime::<Utc>::from_timestamp(timestamp, 0))
}

fn normalize_limit_id(limit_id: String) -> String {
    limit_id.trim().to_ascii_lowercase().replace('-', "_")
}

fn map_usage_status_error(
    account: &ProviderAccount,
    chatgpt_account_id: &str,
    url: &str,
    status: StatusCode,
    body: &[u8],
) -> AppError {
    let tracing_body = response_body_for_tracing(body);
    warn!(
        gpt_account_id = %account.id,
        chatgpt_account_id,
        usage_url = %url,
        upstream_status = status.as_u16(),
        upstream_body_bytes = body.len(),
        upstream_response_body_encoding = tracing_body.encoding(),
        upstream_response_body = %tracing_body.content(),
        "GPT 账号额度接口返回失败状态，完整响应正文已写入 tracing，但不会回显到 Dashboard 响应"
    );

    if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
        return AppError::BadRequest {
            message: "账号 access token 无效或已过期，请等待后台刷新 token 后重试，或重新授权账号"
                .to_owned(),
        };
    }

    AppError::ProviderUpstream {
        provider: PROVIDER.to_owned(),
        message: format!("GPT 账号额度接口返回失败状态: HTTP {status}"),
    }
}
