//! GPT OAuth 账号访问 ChatGPT/Codex 账号级接口时共享的认证与 URL 规则。

use reqwest::{Method, RequestBuilder, header::HeaderName, header::HeaderValue};

use crate::{
    err::{AppError, AppResult},
    provider::{
        credential::ProviderAccount,
        gpt::{codex_http::header as codex_header, model::GptAccountSpecific},
    },
    state::AppState,
};

const CHATGPT_ACCOUNT_ID_HEADER: &str = "chatgpt-account-id";
const FEDRAMP_HEADER: &str = "x-openai-fedramp";

/// 已从持久账号中校验并提取的 ChatGPT 请求认证信息。
///
/// 额度查询和人工重置必须使用完全相同的 workspace 路由。集中构造请求可以防止后续新增
/// 账号级接口时遗漏 `ChatGPT-Account-ID`、FedRAMP 或 Codex 客户端身份头。
pub(super) struct GptAccountApiAuth<'a> {
    access_token: &'a str,
    chatgpt_account_id: String,
    fedramp: bool,
}

impl<'a> GptAccountApiAuth<'a> {
    pub(super) fn from_account(
        account: &'a ProviderAccount,
        operation: &'static str,
    ) -> AppResult<Self> {
        let specific = account.parse_specific::<GptAccountSpecific>()?;
        let access_token = account.access_token.trim();
        if access_token.is_empty() {
            return Err(AppError::BadRequest {
                message: format!("GPT 账号缺少 access_token，无法{operation}"),
            });
        }

        let chatgpt_account_id = specific
            .chatgpt_account_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| AppError::BadRequest {
                message: format!("GPT 账号缺少 chatgpt_account_id，无法按账号{operation}"),
            })?
            .to_owned();

        Ok(Self {
            access_token,
            chatgpt_account_id,
            fedramp: specific.chatgpt_account_is_fedramp,
        })
    }

    pub(super) fn chatgpt_account_id(&self) -> &str {
        &self.chatgpt_account_id
    }

    pub(super) fn is_fedramp(&self) -> bool {
        self.fedramp
    }

    /// 构造带完整 ChatGPT workspace 路由信息的请求。
    pub(super) fn request(&self, state: &AppState, method: Method, url: &str) -> RequestBuilder {
        let mut request = state
            .chatgpt_codex_http_client()
            .request(method, url)
            // 账号级接口没有下游请求 UA，使用与模型请求规范化逻辑相同的固定身份。
            .header(
                reqwest::header::USER_AGENT,
                HeaderValue::from_static(codex_header::FALLBACK_CODEX_USER_AGENT),
            )
            .header(
                HeaderName::from_static(codex_header::ORIGINATOR_HEADER),
                HeaderValue::from_static(codex_header::FALLBACK_CODEX_CLIENT),
            )
            .bearer_auth(self.access_token)
            .header(
                HeaderName::from_static(CHATGPT_ACCOUNT_ID_HEADER),
                &self.chatgpt_account_id,
            );
        if self.fedramp {
            request = request.header(
                HeaderName::from_static(FEDRAMP_HEADER),
                HeaderValue::from_static("true"),
            );
        }
        request
    }
}

/// 根据配置的 Codex base URL 选择生产 `/wham` 或兼容服务 `/api/codex` 路径。
pub(super) fn account_api_url(
    upstream_base_url: &str,
    chatgpt_path: &str,
    codex_api_path: &str,
) -> String {
    let mut base = upstream_base_url.trim().trim_end_matches('/').to_owned();
    if let Some(stripped) = base.strip_suffix("/codex") {
        base = stripped.to_owned();
    }

    if base.contains("/backend-api") {
        format!("{base}/wham/{chatgpt_path}")
    } else {
        format!("{base}/api/codex/{codex_api_path}")
    }
}
