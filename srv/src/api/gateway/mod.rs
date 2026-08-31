mod auth;
mod endpoint;
mod pipeline;

use axum::{
    Router,
    body::Body,
    extract::{OriginalUri, Request, State},
    http::Response,
    response::IntoResponse,
    routing::post,
};
use tracing::{info, instrument};

use crate::{
    provider::{
        claude::messages::ClaudeMessagesProxy, gpt::image_edits::GptImageEditsProxy,
        gpt::image_generations::GptImageGenerationsProxy, gpt::responses::GptResponsesProxy,
        gpt::search::GptSearchProxy,
    },
    request::events::RequestEvent,
    state::AppState,
};

use self::{
    endpoint::{
        EndpointDescriptor, IMAGE_EDITS_ROUTE, IMAGE_GENERATIONS_ROUTE,
        MESSAGES_COUNT_TOKENS_ROUTE, MESSAGES_ROUTE, OperationId, RESPONSES_ROUTE, SEARCH_ROUTE,
    },
    pipeline::execute_pipeline,
};

/// Provider 模型网关路由。
///
/// 每条已知路由只声明 HTTP method/path，实际 provider 与 operation 在公共 handler 内
/// 识别。这样路由不再直接进入 GPT Responses handler，后续 provider 共用完整生命周期。
pub fn router() -> Router<AppState> {
    Router::new()
        .route(RESPONSES_ROUTE, post(handle_provider_request))
        .route(SEARCH_ROUTE, post(handle_provider_request))
        .route(IMAGE_GENERATIONS_ROUTE, post(handle_provider_request))
        .route(IMAGE_EDITS_ROUTE, post(handle_provider_request))
        .route(MESSAGES_ROUTE, post(handle_provider_request))
        .route(MESSAGES_COUNT_TOKENS_ROUTE, post(handle_provider_request))
}

#[instrument(skip_all, fields(component = "provider_gateway"))]
async fn handle_provider_request(
    State(state): State<AppState>,
    OriginalUri(uri): OriginalUri,
    request: Request<Body>,
) -> Response<Body> {
    let endpoint = match EndpointDescriptor::identify(request.method(), &uri) {
        Ok(endpoint) => endpoint,
        Err(error) => return error.into_response(),
    };
    // 该 ID 是模型请求在网关内部的唯一关联标识。一次请求即使发生多次上游重试，
    // lifecycle、scheduler、maintenance 与请求日志也始终复用同一个 ID。
    let request_id = uuid::Uuid::now_v7();
    state.request_events().emit(RequestEvent::Started {
        request_id,
        provider: endpoint.provider,
        route: endpoint.route,
        occurred_at: chrono::Utc::now(),
    });

    info!(
        request_id = %request_id,
        provider = endpoint.provider,
        operation = endpoint.operation.as_str(),
        route = endpoint.route,
        method = %request.method(),
        uri = %uri,
        "模型请求已进入通用 provider gateway"
    );

    match endpoint.operation {
        OperationId::Responses => {
            execute_pipeline::<GptResponsesProxy>(&state, endpoint, uri, request, request_id).await
        }
        OperationId::Search => {
            execute_pipeline::<GptSearchProxy>(&state, endpoint, uri, request, request_id).await
        }
        OperationId::ImageGenerations => {
            execute_pipeline::<GptImageGenerationsProxy>(&state, endpoint, uri, request, request_id)
                .await
        }
        OperationId::ImageEdits => {
            execute_pipeline::<GptImageEditsProxy>(&state, endpoint, uri, request, request_id).await
        }
        OperationId::Messages | OperationId::MessagesCountTokens => {
            execute_pipeline::<ClaudeMessagesProxy>(&state, endpoint, uri, request, request_id)
                .await
        }
    }
}
