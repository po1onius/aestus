use axum::http::{Method, Uri};

use crate::err::{AppError, AppResult};

pub(super) const RESPONSES_ROUTE: &str = "/v1/responses";
pub(super) const SEARCH_ROUTE: &str = "/v1/alpha/search";
pub(super) const IMAGE_GENERATIONS_ROUTE: &str = "/v1/images/generations";
pub(super) const IMAGE_EDITS_ROUTE: &str = "/v1/images/edits";
pub(super) const MESSAGES_ROUTE: &str = "/v1/messages";
pub(super) const MESSAGES_COUNT_TOKENS_ROUTE: &str = "/v1/messages/count_tokens";

/// 通用请求流程识别出的 provider 操作。
///
/// 路由层仍显式注册已知 HTTP 接口，以保留 Axum 的正常 404/405 行为；所有接口随后都
/// 进入同一个 handler，并在这里按 method/path 解析 operation，而不是直接绑定 provider
/// 业务 handler。新增 provider 或同一 provider 的新操作时只需增加 descriptor 和 adapter。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OperationId {
    Responses,
    Search,
    ImageGenerations,
    ImageEdits,
    Messages,
    MessagesCountTokens,
}

impl OperationId {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Responses => "responses",
            Self::Search => "search",
            Self::ImageGenerations => "image_generations",
            Self::ImageEdits => "image_edits",
            Self::Messages => "messages",
            Self::MessagesCountTokens => "messages_count_tokens",
        }
    }
}

/// endpoint 对账号插件套件的使用策略。
///
/// 当前 Dashboard 三个插件插槽只定义了 Responses/Messages 风格的通用 ABI；Search
/// 要求原始 JSON 直接透传，Images 的普通 Rust 转换函数也尚未包装成 Component。由 endpoint
/// 显式声明策略，可以避免继续用 URI 白名单暗示插件能力，也能防止绑定了 Responses 插件的
/// API Key 误处理搜索或图片请求。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PluginPolicy {
    Disabled,
    AccountAttempts,
}

impl PluginPolicy {
    pub(super) const fn enabled(self) -> bool {
        matches!(self, Self::AccountAttempts)
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct EndpointDescriptor {
    pub(super) provider: &'static str,
    pub(super) operation: OperationId,
    pub(super) route: &'static str,
    pub(super) plugin_policy: PluginPolicy,
}

impl EndpointDescriptor {
    pub(super) fn identify(method: &Method, uri: &Uri) -> AppResult<Self> {
        let operation = match (method, uri.path()) {
            (&Method::POST, RESPONSES_ROUTE) => OperationId::Responses,
            (&Method::POST, SEARCH_ROUTE) => OperationId::Search,
            (&Method::POST, IMAGE_GENERATIONS_ROUTE) => OperationId::ImageGenerations,
            (&Method::POST, IMAGE_EDITS_ROUTE) => OperationId::ImageEdits,
            (&Method::POST, MESSAGES_ROUTE) => OperationId::Messages,
            (&Method::POST, MESSAGES_COUNT_TOKENS_ROUTE) => OperationId::MessagesCountTokens,
            _ => {
                return Err(AppError::BadRequest {
                    message: format!("通用 gateway 无法识别 endpoint: {method} {}", uri.path()),
                });
            }
        };

        Ok(Self {
            provider: match operation {
                OperationId::Messages | OperationId::MessagesCountTokens => {
                    crate::provider::claude::model::PROVIDER
                }
                OperationId::Responses
                | OperationId::Search
                | OperationId::ImageGenerations
                | OperationId::ImageEdits => crate::provider::gpt::model::PROVIDER,
            },
            operation,
            route: match operation {
                OperationId::Responses => RESPONSES_ROUTE,
                OperationId::Search => SEARCH_ROUTE,
                OperationId::ImageGenerations => IMAGE_GENERATIONS_ROUTE,
                OperationId::ImageEdits => IMAGE_EDITS_ROUTE,
                OperationId::Messages => MESSAGES_ROUTE,
                OperationId::MessagesCountTokens => MESSAGES_COUNT_TOKENS_ROUTE,
            },
            plugin_policy: match operation {
                OperationId::Responses | OperationId::Messages => PluginPolicy::AccountAttempts,
                OperationId::Search
                | OperationId::ImageGenerations
                | OperationId::ImageEdits
                | OperationId::MessagesCountTokens => PluginPolicy::Disabled,
            },
        })
    }
}
