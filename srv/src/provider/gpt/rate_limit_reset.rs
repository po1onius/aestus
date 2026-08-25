//! ChatGPT/Codex 账号已获得的额度重置凭证查询与兑换。

use reqwest::{Method, RequestBuilder, StatusCode, header::ACCEPT, header::HeaderValue};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tracing::{info, warn};
use uuid::Uuid;

use crate::{
    err::{AppError, AppResult},
    provider::{
        credential::ProviderAccount,
        gpt::{
            account_api::{GptAccountApiAuth, account_api_url},
            model::PROVIDER,
        },
        response_logging::response_body_for_tracing,
    },
    state::AppState,
};

/// Dashboard 展示的一次可兑换额度重置记录。
///
/// 时间字段保留上游 RFC 3339 文本，避免网关在管理展示链路中改写精度或时区。前端只负责
/// 格式化显示，真正兑换始终使用不透明的 `id`。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RateLimitResetCredit {
    pub id: String,
    pub reset_type: String,
    pub status: String,
    pub granted_at: String,
    pub expires_at: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
}

/// 查询可用重置次数接口的完整 Dashboard 响应。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RateLimitResetCreditsResponse {
    pub credits: Vec<RateLimitResetCredit>,
    pub available_count: i64,
}

/// Dashboard 应用某条重置记录时只需提交上游返回的不透明 ID；幂等键由后端生成。
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsumeRateLimitResetCreditRequest {
    pub credit_id: String,
}

#[derive(Debug, Serialize)]
struct UpstreamConsumeRequest<'a> {
    redeem_request_id: &'a Uuid,
    credit_id: &'a str,
}

/// 上游兑换结果。未知的新状态必须显式适配，不能被误判为兑换成功。
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConsumeRateLimitResetCreditCode {
    Reset,
    NothingToReset,
    NoCredit,
    AlreadyRedeemed,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ConsumeRateLimitResetCreditResponse {
    pub code: ConsumeRateLimitResetCreditCode,
    #[serde(default)]
    pub windows_reset: i64,
}

/// 查询账号当前可兑换的额度重置记录。
pub async fn fetch_rate_limit_reset_credits(
    state: &AppState,
    account: &ProviderAccount,
) -> AppResult<RateLimitResetCreditsResponse> {
    let auth = GptAccountApiAuth::from_account(account, "查询可用重置次数")?;
    let url = reset_credits_url(&state.config().gpt_upstream_base_url);
    info!(
        gpt_account_id = %account.id,
        chatgpt_account_id = auth.chatgpt_account_id(),
        reset_credits_url = %url,
        fedramp = auth.is_fedramp(),
        "开始查询 GPT 账号可用额度重置记录"
    );

    let request = auth
        .request(state, Method::GET, &url)
        .header(ACCEPT, HeaderValue::from_static("application/json"));
    let payload = execute_json_request::<RateLimitResetCreditsResponse>(
        request,
        account,
        auth.chatgpt_account_id(),
        &url,
        "查询额度重置记录",
    )
    .await?;

    info!(
        gpt_account_id = %account.id,
        chatgpt_account_id = auth.chatgpt_account_id(),
        available_count = payload.available_count,
        credit_count = payload.credits.len(),
        "GPT 账号可用额度重置记录查询完成"
    );
    Ok(payload)
}

/// 使用指定的重置记录。幂等键由 Dashboard 后端为本次应用操作生成并传入。
pub async fn consume_rate_limit_reset_credit(
    state: &AppState,
    account: &ProviderAccount,
    idempotency_key: Uuid,
    credit_id: &str,
) -> AppResult<ConsumeRateLimitResetCreditResponse> {
    let credit_id = credit_id.trim();
    if credit_id.is_empty() {
        return Err(AppError::BadRequest {
            message: "额度重置记录 ID 不能为空".to_owned(),
        });
    }
    if credit_id.len() > 4_096 {
        return Err(AppError::BadRequest {
            message: "额度重置记录 ID 不能超过 4096 字节".to_owned(),
        });
    }

    let auth = GptAccountApiAuth::from_account(account, "应用额度重置")?;
    let url = consume_reset_credit_url(&state.config().gpt_upstream_base_url);
    info!(
        gpt_account_id = %account.id,
        chatgpt_account_id = auth.chatgpt_account_id(),
        credit_id,
        idempotency_key = %idempotency_key,
        consume_reset_credit_url = %url,
        fedramp = auth.is_fedramp(),
        "开始应用 GPT 账号额度重置记录"
    );

    let request = auth
        .request(state, Method::POST, &url)
        .header(ACCEPT, HeaderValue::from_static("application/json"))
        .json(&UpstreamConsumeRequest {
            redeem_request_id: &idempotency_key,
            credit_id,
        });
    let payload = execute_json_request::<ConsumeRateLimitResetCreditResponse>(
        request,
        account,
        auth.chatgpt_account_id(),
        &url,
        "应用额度重置记录",
    )
    .await?;

    info!(
        gpt_account_id = %account.id,
        chatgpt_account_id = auth.chatgpt_account_id(),
        credit_id,
        idempotency_key = %idempotency_key,
        outcome = ?payload.code,
        windows_reset = payload.windows_reset,
        "GPT 账号额度重置记录应用完成"
    );
    Ok(payload)
}

async fn execute_json_request<T: DeserializeOwned>(
    request: RequestBuilder,
    account: &ProviderAccount,
    chatgpt_account_id: &str,
    url: &str,
    operation: &'static str,
) -> AppResult<T> {
    let response = request.send().await.map_err(|source| {
        warn!(
            gpt_account_id = %account.id,
            chatgpt_account_id,
            upstream_url = %url,
            operation,
            error = %source,
            "请求 GPT 账号额度重置接口失败"
        );
        AppError::ProviderUpstream {
            provider: PROVIDER.to_owned(),
            message: format!("{operation}接口请求失败: {source}"),
        }
    })?;

    let status = response.status();
    let body = response.bytes().await.map_err(|source| {
        warn!(
            gpt_account_id = %account.id,
            chatgpt_account_id,
            upstream_url = %url,
            operation,
            error = %source,
            "读取 GPT 账号额度重置响应体失败"
        );
        AppError::ProviderUpstream {
            provider: PROVIDER.to_owned(),
            message: format!("读取{operation}响应体失败: {source}"),
        }
    })?;

    if !status.is_success() {
        return Err(map_reset_status_error(
            account,
            chatgpt_account_id,
            url,
            operation,
            status,
            &body,
        ));
    }

    serde_json::from_slice::<T>(&body).map_err(|source| {
        let tracing_body = response_body_for_tracing(&body);
        warn!(
            gpt_account_id = %account.id,
            chatgpt_account_id,
            upstream_url = %url,
            operation,
            error = %source,
            response_bytes = body.len(),
            upstream_response_body_encoding = tracing_body.encoding(),
            upstream_response_body = %tracing_body.content(),
            "GPT 账号额度重置响应 JSON 解析失败，完整响应正文已写入 tracing"
        );
        AppError::ProviderUpstream {
            provider: PROVIDER.to_owned(),
            message: format!("{operation}响应格式无效: {source}"),
        }
    })
}

fn reset_credits_url(upstream_base_url: &str) -> String {
    account_api_url(
        upstream_base_url,
        "rate-limit-reset-credits",
        "rate-limit-reset-credits",
    )
}

fn consume_reset_credit_url(upstream_base_url: &str) -> String {
    account_api_url(
        upstream_base_url,
        "rate-limit-reset-credits/consume",
        "rate-limit-reset-credits/consume",
    )
}

fn map_reset_status_error(
    account: &ProviderAccount,
    chatgpt_account_id: &str,
    url: &str,
    operation: &'static str,
    status: StatusCode,
    body: &[u8],
) -> AppError {
    let tracing_body = response_body_for_tracing(body);
    warn!(
        gpt_account_id = %account.id,
        chatgpt_account_id,
        upstream_url = %url,
        operation,
        upstream_status = status.as_u16(),
        upstream_body_bytes = body.len(),
        upstream_response_body_encoding = tracing_body.encoding(),
        upstream_response_body = %tracing_body.content(),
        "GPT 账号额度重置接口返回失败状态，完整响应正文已写入 tracing"
    );

    if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
        return AppError::BadRequest {
            message: "账号 access token 无效或已过期，请等待后台刷新 token 后重试，或重新授权账号"
                .to_owned(),
        };
    }

    AppError::ProviderUpstream {
        provider: PROVIDER.to_owned(),
        message: format!("{operation}接口返回失败状态: HTTP {status}"),
    }
}
