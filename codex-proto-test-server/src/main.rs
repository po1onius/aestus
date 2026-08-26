use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use axum::{
    Router,
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use clap::Parser;
use gpt_codex_plugin_common::{
    Effects, Header,
    functions::{
        AccountResource, BufferedDisposition, BufferedTransformInput, HttpResponse,
        RequestTransformInput, ResponseHead, ResponseMode, StreamResponseTransformer,
        StreamStartInput, transform_buffered_response, transform_request,
    },
    sse::{JsonSseItem, split_sse_items},
};
use reqwest::Client;
use serde_json::{Value, json};
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

const DEFAULT_UPSTREAM_URL: &str = "https://chatgpt.com/backend-api/codex/responses";
const DEFAULT_OAUTH_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const DEFAULT_OAUTH_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const MAX_REQUEST_BYTES: usize = 512 * 1024 * 1024;

#[derive(Debug, Parser)]
#[command(
    name = "codex-proto-test-server",
    about = "通过 Codex 账号端点验证 Responses 协议转换"
)]
struct Arguments {
    /// Codex 账号登录产生的 access token。传入后优先使用，不再消费 refresh token。
    #[arg(long, value_name = "TOKEN")]
    access_token: Option<String>,

    /// 未提供 access token 时，用该 refresh token 向 OpenAI OAuth 端点换取 access token。
    #[arg(long, value_name = "TOKEN")]
    refresh_token: Option<String>,

    /// OAuth 刷新请求使用的 client ID；默认与 Codex CLI 当前内置值保持一致。
    #[arg(long, default_value = DEFAULT_OAUTH_CLIENT_ID, value_name = "CLIENT_ID")]
    client_id: String,

    /// 多账号场景使用的 ChatGPT account ID；单账号 token 通常可以不传。
    #[arg(long, value_name = "ACCOUNT_ID")]
    chatgpt_account_id: Option<String>,

    /// 测试服务监听地址。
    #[arg(long, default_value = "127.0.0.1:3000")]
    listen: SocketAddr,

    /// Codex Responses 上游地址。该参数主要用于本地抓包或替身服务调试。
    #[arg(long, default_value = DEFAULT_UPSTREAM_URL)]
    upstream_url: String,

    /// 单次上游请求总超时秒数。
    #[arg(long, default_value_t = 600)]
    upstream_timeout_seconds: u64,
}

#[derive(Clone)]
struct AppState {
    client: Client,
    access_token: Arc<str>,
    chatgpt_account_id: Option<Arc<str>>,
    responses_upstream_url: Arc<str>,
    request_sequence: Arc<AtomicU64>,
}

#[tokio::main]
async fn main() {
    init_logging();
    let arguments = Arguments::parse();

    let client = match Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(arguments.upstream_timeout_seconds))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            error!(error = %error, "创建上游 HTTP 客户端失败");
            std::process::exit(1);
        }
    };

    // 显式 access token 始终优先，避免调用方同时传入两种凭证时意外消费可能轮换的
    // refresh token。只有 access token 缺失或全为空白时才执行 OAuth 刷新。
    let (access_token, authentication_source) =
        match resolve_access_token(&client, &arguments).await {
            Ok(resolved) => resolved,
            Err(TokenResolutionError::InvalidArguments(message)) => {
                error!(error = %message, "启动参数中的 OAuth 凭证无效");
                std::process::exit(2);
            }
            Err(TokenResolutionError::RefreshFailed(message)) => {
                error!(error = %message, "使用 refresh token 获取 access token 失败");
                std::process::exit(1);
            }
        };
    let state = AppState {
        client,
        access_token: Arc::from(access_token),
        chatgpt_account_id: arguments
            .chatgpt_account_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(Arc::from),
        responses_upstream_url: Arc::from(arguments.upstream_url.as_str()),
        request_sequence: Arc::new(AtomicU64::new(1)),
    };
    let app = Router::new()
        .route("/healthz", get(health))
        .route("/v1/responses", post(create_response))
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BYTES))
        .with_state(state);

    let listener = match tokio::net::TcpListener::bind(arguments.listen).await {
        Ok(listener) => listener,
        Err(error) => {
            error!(listen = %arguments.listen, error = %error, "绑定监听地址失败");
            std::process::exit(1);
        }
    };
    info!(
        listen = %arguments.listen,
        responses_upstream = %arguments.upstream_url,
        account_id_present = arguments.chatgpt_account_id.is_some(),
        authentication_source,
        "Codex 协议测试服务已启动"
    );
    if let Err(error) = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
    {
        error!(error = %error, "HTTP 服务异常退出");
        std::process::exit(1);
    }
}

#[derive(Debug)]
enum TokenResolutionError {
    InvalidArguments(String),
    RefreshFailed(String),
}

/// 根据启动参数解析服务实际使用的 access token。
///
/// refresh token 属于高敏感凭证，整个流程只按引用传递给 reqwest 的 JSON 序列化器，
/// 错误与日志均不包含请求正文或 token 原文。
async fn resolve_access_token(
    client: &Client,
    arguments: &Arguments,
) -> Result<(String, &'static str), TokenResolutionError> {
    if let Some(access_token) = arguments
        .access_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if arguments
            .refresh_token
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
        {
            info!("同时提供了 access token 和 refresh token，将优先使用 access token");
        }
        return Ok((access_token.to_owned(), "access-token"));
    }

    let refresh_token = arguments
        .refresh_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            TokenResolutionError::InvalidArguments(
                "必须提供非空 --access-token 或 --refresh-token".to_owned(),
            )
        })?;
    let client_id = arguments.client_id.trim();
    if client_id.is_empty() {
        return Err(TokenResolutionError::InvalidArguments(
            "使用 --refresh-token 时，--client-id 不能为空".to_owned(),
        ));
    }

    info!(
        oauth_token_url = DEFAULT_OAUTH_TOKEN_URL,
        client_id, "未提供 access token，开始使用 refresh token 获取 access token"
    );
    let access_token = exchange_refresh_token(client, refresh_token, client_id)
        .await
        .map_err(TokenResolutionError::RefreshFailed)?;
    Ok((access_token, "refresh-token"))
}

/// 按 Codex CLI 当前协议向 OpenAI OAuth token 端点发送 JSON 刷新请求。
async fn exchange_refresh_token(
    client: &Client,
    refresh_token: &str,
    client_id: &str,
) -> Result<String, String> {
    let started_at = Instant::now();
    let response = client
        .post(DEFAULT_OAUTH_TOKEN_URL)
        .json(&json!({
            "client_id": client_id,
            "grant_type": "refresh_token",
            "refresh_token": refresh_token,
        }))
        .send()
        .await
        .map_err(|error| format!("请求 OAuth token 端点失败: {error}"))?;
    let status = response.status();
    let body = response
        .bytes()
        .await
        .map_err(|error| format!("读取 OAuth token 响应失败: {error}"))?;

    info!(
        oauth_status = status.as_u16(),
        response_bytes = body.len(),
        elapsed_ms = started_at.elapsed().as_millis(),
        "已收到 OAuth token 响应"
    );
    if !status.is_success() {
        return Err(summarize_oauth_error(status.as_u16(), &body));
    }

    let value: Value = serde_json::from_slice(&body)
        .map_err(|error| format!("OAuth token 成功响应不是合法 JSON: {error}"))?;
    let access_token = value
        .get("access_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "OAuth token 成功响应缺少非空 access_token".to_owned())?;
    let rotated_refresh_token_present = value
        .get("refresh_token")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty());
    info!(
        rotated_refresh_token_present,
        "已通过 refresh token 获取 access token"
    );
    Ok(access_token.to_owned())
}

/// 只提取 OAuth 错误码并映射为固定说明，避免服务端错误正文意外回显凭证后进入日志。
fn summarize_oauth_error(status: u16, body: &[u8]) -> String {
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return format!("OAuth token 端点返回 HTTP {status}，响应体不是合法 JSON");
    };
    let code = value
        .pointer("/error/code")
        .and_then(Value::as_str)
        .or_else(|| value.get("error").and_then(Value::as_str))
        .or_else(|| value.get("code").and_then(Value::as_str))
        .unwrap_or("unknown");
    let message = match code.to_ascii_lowercase().as_str() {
        "invalid_grant" => "refresh token 无效或已不可用",
        "refresh_token_expired" => "refresh token 已过期",
        "refresh_token_reused" => "refresh token 已被使用",
        "refresh_token_invalidated" => "refresh token 已被撤销",
        "invalid_client" => "client ID 无效",
        _ => "OAuth 服务返回未识别错误",
    };
    format!("OAuth token 端点返回 HTTP {status}: code={code}, message={message}")
}

fn init_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();
}

async fn shutdown_signal() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        error!(error = %error, "监听 Ctrl-C 失败");
        return;
    }
    info!("收到退出信号，正在停止测试服务");
}

async fn health() -> impl IntoResponse {
    axum::Json(json!({"status": "ok"}))
}

async fn create_response(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ServiceError> {
    let request_id = state.request_sequence.fetch_add(1, Ordering::Relaxed);
    let request_summary = summarize_request(&body);
    let model = request_summary
        .model
        .as_deref()
        .unwrap_or(if request_summary.valid_json {
            "<missing>"
        } else {
            "<invalid-json>"
        });
    info!(
        request_id,
        model,
        downstream_stream = request_summary.stream,
        request_bytes = body.len(),
        "收到 Responses 测试请求"
    );

    let transformed_request = transform_request(RequestTransformInput {
        account: AccountResource {
            access_token: state.access_token.to_string(),
            chatgpt_account_id: state.chatgpt_account_id.as_deref().map(str::to_owned),
            chatgpt_account_is_fedramp: false,
        },
        headers: from_axum_headers(&headers),
        body: body.to_vec(),
    })
    .map_err(|error| {
        warn!(request_id, code = %error.code, message = %error.message, "请求插件函数拒绝请求");
        ServiceError::bad_request(error.code, error.message)
    })?;

    let started_at = Instant::now();
    let upstream = send_upstream(
        &state,
        state.responses_upstream_url.as_ref(),
        &transformed_request.headers,
        transformed_request.body,
    )
    .await
    .map_err(|error| {
        error!(request_id, error = %error, "请求 Codex 上游失败");
        ServiceError::bad_gateway("upstream_request_failed", error.to_string())
    })?;
    info!(
        request_id,
        upstream_status = upstream.status,
        upstream_bytes = upstream.body.len(),
        elapsed_ms = started_at.elapsed().as_millis(),
        "已收到 Codex 上游响应"
    );

    // 与真实宿主一致：非 2xx 永远走 buffered；成功响应才使用请求插件声明的模式。
    if !(200..300).contains(&upstream.status)
        || transformed_request.response_mode == ResponseMode::Buffered
    {
        handle_buffered_response(request_id, upstream, transformed_request.response_context)
    } else {
        handle_stream_response(request_id, upstream, transformed_request.response_context)
    }
}

async fn send_upstream(
    state: &AppState,
    upstream_url: &str,
    headers: &[Header],
    body: Vec<u8>,
) -> Result<HttpResponse, reqwest::Error> {
    let response = state
        .client
        .post(upstream_url)
        .headers(to_reqwest_headers(headers))
        .body(body)
        .send()
        .await?;
    let status = response.status().as_u16();
    let headers = from_reqwest_headers(response.headers());
    let body = response.bytes().await?.to_vec();
    Ok(HttpResponse {
        status,
        headers,
        body,
    })
}

fn handle_buffered_response(
    request_id: u64,
    response: HttpResponse,
    request_context: Option<Vec<u8>>,
) -> Result<Response, ServiceError> {
    let transformed = transform_buffered_response(BufferedTransformInput {
        response,
        request_context,
    })
    .map_err(|error| {
        error!(request_id, code = %error.code, message = %error.message, "缓冲响应插件函数执行失败");
        ServiceError::bad_gateway(error.code, error.message)
    })?;
    log_effects(request_id, &transformed.effects);
    let BufferedDisposition::Respond(response) = transformed.disposition;
    validate_json_response(response.status, &response.body).map_err(|message| {
        error!(request_id, validation_error = %message, "插件输出不是合法 Responses JSON");
        ServiceError::bad_gateway("invalid_responses_json", message)
    })?;
    info!(
        request_id,
        status = response.status,
        "Responses JSON 格式校验通过"
    );
    build_response(response)
}

fn handle_stream_response(
    request_id: u64,
    response: HttpResponse,
    request_context: Option<Vec<u8>>,
) -> Result<Response, ServiceError> {
    let HttpResponse {
        status,
        headers,
        body,
    } = response;
    let mut transformer = StreamResponseTransformer::default();
    let head = transformer
        .start(StreamStartInput {
            head: ResponseHead { status, headers },
            request_context,
        })
        .map_err(|error| ServiceError::bad_gateway(error.code, error.message))?;
    let items = split_sse_items(&body)
        .map_err(|message| ServiceError::bad_gateway("invalid_upstream_sse", message))?;
    let mut transformed_body = Vec::with_capacity(body.len());
    for item in items {
        let transformed = transformer.transform_item(item).map_err(|error| {
            error!(request_id, code = %error.code, message = %error.message, "流式响应插件函数执行失败");
            ServiceError::bad_gateway(error.code, error.message)
        })?;
        log_effects(request_id, &transformed.effects);
        if let Some(item) = transformed.item {
            transformed_body.extend_from_slice(&item);
        }
    }
    let finished = transformer
        .finish()
        .map_err(|error| ServiceError::bad_gateway(error.code, error.message))?;
    log_effects(request_id, &finished.effects);
    for item in finished.items {
        transformed_body.extend_from_slice(&item);
    }

    validate_sse_response(&transformed_body).map_err(|message| {
        error!(request_id, validation_error = %message, "插件输出不是合法 Responses SSE");
        ServiceError::bad_gateway("invalid_responses_sse", message)
    })?;
    info!(
        request_id,
        event_stream_bytes = transformed_body.len(),
        "Responses SSE 格式校验通过"
    );
    build_response(HttpResponse {
        status: head.status,
        headers: head.headers,
        body: transformed_body,
    })
}

fn validate_json_response(status: u16, body: &[u8]) -> Result<(), String> {
    let value: Value =
        serde_json::from_slice(body).map_err(|error| format!("响应体不是合法 JSON: {error}"))?;
    if (200..300).contains(&status) {
        validate_response_object(&value, true)
    } else {
        let message = value.pointer("/error/message").and_then(Value::as_str);
        if message.is_some_and(|message| !message.trim().is_empty()) {
            Ok(())
        } else {
            Err("非成功响应缺少 error.message".to_owned())
        }
    }
}

fn validate_sse_response(body: &[u8]) -> Result<(), String> {
    let items = split_sse_items(body)?;
    let mut event_count = 0_u64;
    let mut terminal_count = 0_u64;
    for item in items {
        let Some(parsed) = JsonSseItem::parse(&item)? else {
            continue;
        };
        event_count += 1;
        let event_type = required_string(parsed.value(), "type", "SSE event")?;
        if !event_type.starts_with("response.") {
            return Err(format!(
                "SSE event.type 不属于 Responses 协议: {event_type}"
            ));
        }
        if matches!(
            event_type,
            "response.completed" | "response.done" | "response.incomplete" | "response.failed"
        ) {
            terminal_count += 1;
            let response = parsed
                .value()
                .get("response")
                .ok_or_else(|| format!("{event_type} 缺少 response 对象"))?;
            validate_response_object(response, event_type != "response.failed")?;
        }
    }
    if event_count == 0 {
        return Err("SSE 中没有可解析的 Responses JSON 事件".to_owned());
    }
    if terminal_count != 1 {
        return Err(format!(
            "SSE 必须恰好包含一个终止事件，实际为 {terminal_count}"
        ));
    }
    Ok(())
}

fn validate_response_object(value: &Value, require_output: bool) -> Result<(), String> {
    let object = value
        .as_object()
        .ok_or_else(|| "response 必须是 JSON object".to_owned())?;
    let id = required_string(value, "id", "response")?;
    if !id.starts_with("resp_") {
        return Err(format!("response.id 必须以 resp_ 开头，实际为 {id}"));
    }
    if required_string(value, "object", "response")? != "response" {
        return Err("response.object 必须为 response".to_owned());
    }
    let status = required_string(value, "status", "response")?;
    if !matches!(
        status,
        "completed" | "incomplete" | "failed" | "in_progress" | "cancelled" | "queued"
    ) {
        return Err(format!("response.status 非法: {status}"));
    }
    if require_output || object.contains_key("output") {
        let output = value
            .get("output")
            .and_then(Value::as_array)
            .ok_or_else(|| "response.output 必须是 array".to_owned())?;
        for (index, item) in output.iter().enumerate() {
            validate_output_item(index, item)?;
        }
    }
    if let Some(usage) = object.get("usage").filter(|usage| !usage.is_null()) {
        validate_usage(usage)?;
    }
    Ok(())
}

fn validate_output_item(index: usize, value: &Value) -> Result<(), String> {
    let kind = required_string(value, "type", &format!("output[{index}]"))?;
    match kind {
        "message" => {
            required_string(value, "id", &format!("output[{index}] message"))?;
            if required_string(value, "role", &format!("output[{index}] message"))? != "assistant" {
                return Err(format!("output[{index}] message.role 必须为 assistant"));
            }
            value
                .get("content")
                .and_then(Value::as_array)
                .ok_or_else(|| format!("output[{index}] message.content 必须是 array"))?;
        }
        "function_call" => {
            for field in ["id", "call_id", "name", "arguments"] {
                required_string(value, field, &format!("output[{index}] function_call"))?;
            }
        }
        _ => {
            // Responses 会持续增加内建工具 item；未知类型只验证公共的 type，避免测试
            // 服务因协议扩展把一个由真实上游返回的合法新 item 误判为失败。
        }
    }
    Ok(())
}

fn validate_usage(value: &Value) -> Result<(), String> {
    let usage = value
        .as_object()
        .ok_or_else(|| "response.usage 必须是 object 或 null".to_owned())?;
    for field in ["input_tokens", "output_tokens", "total_tokens"] {
        let valid = usage
            .get(field)
            .and_then(Value::as_i64)
            .is_some_and(|tokens| tokens >= 0);
        if !valid {
            return Err(format!("response.usage.{field} 必须是非负整数"));
        }
    }
    Ok(())
}

fn required_string<'a>(value: &'a Value, field: &str, owner: &str) -> Result<&'a str, String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("{owner}.{field} 必须是非空字符串"))
}

fn build_response(response: HttpResponse) -> Result<Response, ServiceError> {
    let status = StatusCode::from_u16(response.status)
        .map_err(|error| ServiceError::bad_gateway("invalid_plugin_status", error.to_string()))?;
    let mut output = Response::builder().status(status);
    let output_headers = output
        .headers_mut()
        .ok_or_else(|| ServiceError::internal("构造响应 header 失败"))?;
    for header in response.headers {
        let name = HeaderName::from_bytes(header.name.as_bytes()).map_err(|error| {
            ServiceError::bad_gateway("invalid_plugin_header", error.to_string())
        })?;
        let value = HeaderValue::from_bytes(&header.value).map_err(|error| {
            ServiceError::bad_gateway("invalid_plugin_header", error.to_string())
        })?;
        output_headers.append(name, value);
    }
    output
        .body(Body::from(response.body))
        .map_err(|error| ServiceError::internal(error.to_string()))
}

fn from_axum_headers(headers: &HeaderMap) -> Vec<Header> {
    headers
        .iter()
        .map(|(name, value)| Header {
            name: name.as_str().to_owned(),
            value: value.as_bytes().to_vec(),
        })
        .collect()
}

fn to_reqwest_headers(headers: &[Header]) -> reqwest::header::HeaderMap {
    let mut output = reqwest::header::HeaderMap::new();
    for header in headers {
        let Ok(name) = reqwest::header::HeaderName::from_bytes(header.name.as_bytes()) else {
            warn!(header = %header.name, "请求插件产生了非法 header 名，已忽略");
            continue;
        };
        let Ok(value) = reqwest::header::HeaderValue::from_bytes(&header.value) else {
            warn!(header = %header.name, "请求插件产生了非法 header 值，已忽略");
            continue;
        };
        output.append(name, value);
    }
    output
}

fn from_reqwest_headers(headers: &reqwest::header::HeaderMap) -> Vec<Header> {
    headers
        .iter()
        .map(|(name, value)| Header {
            name: name.as_str().to_owned(),
            value: value.as_bytes().to_vec(),
        })
        .collect()
}

#[derive(Default)]
struct RequestSummary {
    model: Option<String>,
    stream: bool,
    valid_json: bool,
}

fn summarize_request(body: &[u8]) -> RequestSummary {
    let Ok(Value::Object(object)) = serde_json::from_slice::<Value>(body) else {
        return RequestSummary::default();
    };
    RequestSummary {
        model: object
            .get("model")
            .and_then(Value::as_str)
            .map(str::to_owned),
        stream: object.get("stream").and_then(Value::as_bool) == Some(true),
        valid_json: true,
    }
}

fn log_effects(request_id: u64, effects: &Effects) {
    if let Some(usage) = effects.usage {
        info!(
            request_id,
            input_tokens = usage.input_tokens,
            cached_input_tokens = usage.cached_input_tokens,
            output_tokens = usage.output_tokens,
            reasoning_output_tokens = usage.reasoning_output_tokens,
            total_tokens = usage.total_tokens,
            "插件提取到累计 usage"
        );
    }
    if let Some(feedback) = effects.feedback.as_ref() {
        warn!(request_id, feedback = ?feedback, "插件产生上游 maintenance feedback");
    }
    if let Some(failure) = effects.failure.as_ref() {
        warn!(request_id, kind = %failure.kind, message = %failure.message, "插件识别到流式失败事件");
    }
}

#[derive(Debug)]
struct ServiceError {
    status: StatusCode,
    code: String,
    message: String,
}

impl ServiceError {
    fn bad_request(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: code.into(),
            message: message.into(),
        }
    }

    fn bad_gateway(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            code: code.into(),
            message: message.into(),
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "test_server_internal_error".to_owned(),
            message: message.into(),
        }
    }
}

impl IntoResponse for ServiceError {
    fn into_response(self) -> Response {
        let body = json!({
            "error": {
                "type": "plugin_test_error",
                "code": self.code,
                "message": self.message,
            }
        });
        (self.status, axum::Json(body)).into_response()
    }
}
