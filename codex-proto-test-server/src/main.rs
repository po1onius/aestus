use std::{
    fs::{self, File, OpenOptions},
    io::{self, BufWriter, Write},
    net::SocketAddr,
    path::{Path, PathBuf},
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
use base64::{
    Engine,
    engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD},
};
use chrono::{SecondsFormat, Utc};
use clap::Parser;
use gpt_codex_buffered_response_plugin::{
    BufferedDisposition, BufferedTransformInput, Effects as BufferedEffects,
    Header as BufferedHeader, HttpResponse as BufferedHttpResponse, transform_buffered_response,
};
use gpt_codex_plugin_utils::sse::{JsonSseItem, body_has_sse_framing, split_sse_items};
use gpt_codex_request_plugin::{
    AccountResource, Header as RequestHeader, RequestTransformInput, transform_request,
};
use gpt_codex_stream_response_plugin::{
    Effects as StreamEffects, Header as StreamHeader, ResponseHead, StreamResponseTransformer,
    StreamStartInput,
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

const DEFAULT_UPSTREAM_URL: &str = "https://chatgpt.com/backend-api/codex/responses";
const DEFAULT_OAUTH_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const DEFAULT_OAUTH_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const MAX_REQUEST_BYTES: usize = 512 * 1024 * 1024;
const TRACE_DIRECTORY_NAME: &str = "trace";

struct Header {
    name: String,
    value: Vec<u8>,
}

struct HttpResponse {
    status: u16,
    headers: Vec<Header>,
    body: Vec<u8>,
}

#[derive(Debug, Parser)]
#[command(
    name = "codex-proto-test-server",
    about = "通过 Codex 账号端点验证 Responses 协议转换"
)]
struct Arguments {
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

/// 单次下游请求对应一个调试记录文件。文件始终使用 append 模式打开；创建文件时同时使用
/// `create_new`，避免同一秒内的并发请求或服务重启后意外复用已有记录。
///
/// 调试记录只包含请求体、响应体和转换错误，不写入请求 header，因此 OAuth access token、
/// refresh token、下游鉴权信息和账号 header 都不会进入文件。
struct TraceRecorder {
    file: BufWriter<File>,
    path: PathBuf,
}

impl TraceRecorder {
    fn create(request_id: u64) -> io::Result<Self> {
        let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(TRACE_DIRECTORY_NAME);
        fs::create_dir_all(&directory)?;

        // 文件名时间精确到秒且便于人工识别。同一秒有多个请求时添加递增序号，仍保证每个
        // 请求拥有独立文件；`create_new` 使并发创建过程具备原子性。
        let timestamp = Utc::now().format("%Y-%m-%d_%H-%M-%S_UTC").to_string();
        for collision_index in 0_u64.. {
            let file_name = if collision_index == 0 {
                format!("{timestamp}.trace.log")
            } else {
                format!("{timestamp}_{collision_index:02}.trace.log")
            };
            let path = directory.join(file_name);
            match OpenOptions::new().append(true).create_new(true).open(&path) {
                Ok(file) => {
                    // pretty JSON 序列化会产生大量小块写入，使用缓冲写入器避免调试记录对
                    // 大响应产生不必要的系统调用开销；每个逻辑 section 结束时仍显式 flush。
                    let mut recorder = Self {
                        file: BufWriter::new(file),
                        path,
                    };
                    recorder.write_text_section(
                        "调试记录开始",
                        &format!(
                            "request_id: {request_id}\ncreated_at: {}",
                            trace_timestamp()
                        ),
                    )?;
                    return Ok(recorder);
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        unreachable!("文件名碰撞序号不会耗尽")
    }

    fn path(&self) -> &Path {
        &self.path
    }

    /// JSON 合法时以缩进格式写入；不合法时保留原始字节并同时记录反序列化错误，确保请求
    /// 或响应协议损坏时仍能从同一个文件直接定位问题。
    fn write_json_section(&mut self, title: &str, body: &[u8]) -> io::Result<()> {
        self.write_section_header(title)?;
        match serde_json::from_slice::<Value>(body) {
            Ok(value) => {
                serde_json::to_writer_pretty(&mut self.file, &value).map_err(json_io_error)?;
                self.file.write_all(b"\n\n")?;
            }
            Err(error) => {
                writeln!(self.file, "JSON 反序列化失败: {error}")?;
                writeln!(self.file, "原始内容（{} bytes）：", body.len())?;
                self.file.write_all(body)?;
                self.file.write_all(b"\n\n")?;
            }
        }
        self.file.flush()
    }

    fn write_text_section(&mut self, title: &str, content: &str) -> io::Result<()> {
        self.write_section_header(title)?;
        self.file.write_all(content.as_bytes())?;
        self.file.write_all(b"\n\n")?;
        self.file.flush()
    }

    /// 记录上游响应的协议状态。SSE 按空行切成完整 event，再分别反序列化 event 的全部
    /// `data:` 行；单个 event 损坏时会把错误和原始 event 一起写入，但不会改变后续插件
    /// 原本应执行的校验与错误返回。
    fn write_upstream_response(&mut self, status: u16, body: &[u8]) -> io::Result<()> {
        self.write_text_section(
            "Codex 上游响应概要",
            &format!(
                "received_at: {}\nstatus: {status}\nbody_bytes: {}\nsse_framing: {}",
                trace_timestamp(),
                body.len(),
                body_has_sse_framing(body)
            ),
        )?;

        if !body_has_sse_framing(body) {
            return self.write_json_section("Codex 上游非 SSE 响应体", body);
        }

        let items = match split_sse_items(body) {
            Ok(items) => items,
            Err(message) => {
                self.write_text_section(
                    "Codex 上游 SSE 切分失败",
                    &format!("error: {message}\n原始响应体（{} bytes）：", body.len()),
                )?;
                self.file.write_all(body)?;
                self.file.write_all(b"\n\n")?;
                return self.file.flush();
            }
        };

        for (index, item) in items.into_iter().enumerate() {
            let title = format!("Codex 上游完整 SSE event #{}", index + 1);
            self.write_section_header(&title)?;
            match JsonSseItem::parse(&item) {
                Ok(Some(parsed)) => {
                    serde_json::to_writer_pretty(&mut self.file, parsed.value())
                        .map_err(json_io_error)?;
                    self.file.write_all(b"\n\n")?;
                }
                Ok(None) => {
                    writeln!(
                        self.file,
                        "该 event 不包含可反序列化的 JSON data（可能是注释、空 data 或 [DONE]）。"
                    )?;
                    writeln!(self.file, "原始 SSE event（{} bytes）：", item.len())?;
                    self.file.write_all(&item)?;
                    self.file.write_all(b"\n")?;
                }
                Err(message) => {
                    writeln!(self.file, "SSE event JSON 反序列化失败: {message}")?;
                    writeln!(self.file, "原始 SSE event（{} bytes）：", item.len())?;
                    self.file.write_all(&item)?;
                    self.file.write_all(b"\n")?;
                }
            }
        }
        self.file.flush()
    }

    fn write_error(&mut self, stage: &str, code: &str, message: &str) -> io::Result<()> {
        self.write_text_section(
            &format!("处理错误：{stage}"),
            &format!(
                "occurred_at: {}\ncode: {code}\nmessage: {message}",
                trace_timestamp()
            ),
        )
    }

    fn write_section_header(&mut self, title: &str) -> io::Result<()> {
        writeln!(
            self.file,
            "================================================================================"
        )?;
        writeln!(self.file, "[{title}]")?;
        writeln!(
            self.file,
            "--------------------------------------------------------------------------------"
        )
    }
}

fn trace_timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn json_io_error(error: serde_json::Error) -> io::Error {
    io::Error::other(error)
}

/// `reqwest::Error` 的 Display 通常只包含最外层阶段和 URL。调试服务需要保留完整 source
/// 链才能区分 DNS、TCP、TLS、代理及 HTTP body 读取错误；错误链不包含请求 header。
fn format_error_chain(error: &dyn std::error::Error) -> String {
    let mut messages = vec![error.to_string()];
    let mut source = error.source();
    while let Some(error) = source {
        messages.push(error.to_string());
        source = error.source();
    }
    messages.join(" -> ")
}

/// 统一处理调试文件的写入错误。记录功能属于测试服务的核心职责，因此一旦文件无法继续
/// 写入就明确终止当前请求，避免调用方误以为已经生成了一份完整可用的调试记录。
fn write_trace_record(
    trace: &mut TraceRecorder,
    request_id: u64,
    write: impl FnOnce(&mut TraceRecorder) -> io::Result<()>,
) -> Result<(), ServiceError> {
    let trace_path = trace.path().to_path_buf();
    write(trace).map_err(|error| {
        error!(
            request_id,
            trace_path = %trace_path.display(),
            error = %error,
            "写入请求调试记录失败"
        );
        ServiceError::internal(format!("写入请求调试记录失败: {error}"))
    })
}

#[tokio::main]
async fn main() {
    init_logging();
    let arguments = Arguments::parse();
    let token_file_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("token.toml");
    let mut token_config = match load_token_config(&token_file_path) {
        Ok(config) => config,
        Err(message) => {
            error!(path = %token_file_path.display(), error = %message, "读取 token.toml 失败");
            std::process::exit(2);
        }
    };

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

    // token.toml 中的 access token 始终优先，避免已有可用凭证时意外消费可能轮换的
    // refresh token。只有 access token 缺失或全为空白时才执行 OAuth 刷新并回填文件。
    let (access_token, authentication_source) = match resolve_access_token(
        &client,
        &mut token_config,
        &token_file_path,
    )
    .await
    {
        Ok(resolved) => resolved,
        Err(TokenResolutionError::InvalidConfiguration(message)) => {
            error!(path = %token_file_path.display(), error = %message, "token.toml 中的 OAuth 凭证无效");
            std::process::exit(2);
        }
        Err(TokenResolutionError::RefreshFailed(message)) => {
            error!(error = %message, "使用 refresh token 获取 access token 失败");
            std::process::exit(1);
        }
    };
    // CLI 参数只用于临时覆盖，便于多 workspace 调试；常规启动优先使用刷新后回填或
    // token.toml 中保留的账号 ID。两处都为空时保持 None，请求插件不会生成账号 header。
    let argument_account_id = arguments
        .chatgpt_account_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let configured_account_id = match token_config.chatgpt_account_id.trim() {
        "" => None,
        value => Some(value.to_owned()),
    };
    let account_id_source = if argument_account_id.is_some() {
        "command-line"
    } else if configured_account_id.is_some() {
        "token-file"
    } else {
        "absent"
    };
    let chatgpt_account_id = argument_account_id.or(configured_account_id).map(Arc::from);
    let account_id_present = chatgpt_account_id.is_some();
    let state = AppState {
        client,
        access_token: Arc::from(access_token),
        chatgpt_account_id,
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
        account_id_present,
        account_id_source,
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
    InvalidConfiguration(String),
    RefreshFailed(String),
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct TokenConfig {
    access_token: String,
    refresh_token: String,
    client_id: String,
    /// ChatGPT 多 workspace 路由使用的 Account ID。兼容旧版三字段配置；缺失字段按空值
    /// 处理，并在下一次 refresh 成功提取到 ID 时自动回填。
    #[serde(default)]
    chatgpt_account_id: String,
}

#[derive(Debug)]
struct RefreshedTokens {
    access_token: String,
    refresh_token: Option<String>,
    chatgpt_account_id: Option<String>,
}

/// 从测试服务目录读取固定的 token.toml。日志只记录字段是否存在，不打印任何凭证值。
fn load_token_config(path: &Path) -> Result<TokenConfig, String> {
    let content = fs::read_to_string(path).map_err(|error| format!("无法读取凭证文件: {error}"))?;
    let config: TokenConfig =
        toml::from_str(&content).map_err(|error| format!("凭证文件不是合法 TOML: {error}"))?;
    info!(
        path = %path.display(),
        access_token_present = !config.access_token.trim().is_empty(),
        refresh_token_present = !config.refresh_token.trim().is_empty(),
        client_id_present = !config.client_id.trim().is_empty(),
        chatgpt_account_id_present = !config.chatgpt_account_id.trim().is_empty(),
        "已读取 OAuth 凭证文件"
    );
    Ok(config)
}

/// 把刷新后的完整凭证状态写回原文件。`fs::write` 会保留现有 token.toml 的文件权限，
/// 因此不会在业务代码中引入针对不同运行环境的权限兼容分支。
fn persist_token_config(path: &Path, config: &TokenConfig) -> Result<(), String> {
    let content = toml::to_string_pretty(config)
        .map_err(|error| format!("序列化刷新后的凭证失败: {error}"))?;
    fs::write(path, content).map_err(|error| format!("写回凭证文件失败: {error}"))?;
    info!(path = %path.display(), "已把刷新后的 OAuth 凭证写回 token.toml");
    Ok(())
}

/// 根据 token.toml 解析服务实际使用的 access token。
///
/// refresh token 属于高敏感凭证，整个流程只按引用传递给 reqwest 的 JSON 序列化器，
/// 错误与日志均不包含请求正文或 token 原文。
async fn resolve_access_token(
    client: &Client,
    config: &mut TokenConfig,
    token_file_path: &Path,
) -> Result<(String, &'static str), TokenResolutionError> {
    let access_token = config.access_token.trim();
    if !access_token.is_empty() {
        return Ok((access_token.to_owned(), "token-file-access-token"));
    }

    let refresh_token = config.refresh_token.trim().to_owned();
    if refresh_token.is_empty() {
        return Err(TokenResolutionError::InvalidConfiguration(
            "access_token 和 refresh_token 不能同时为空".to_owned(),
        ));
    }
    let client_id = match config.client_id.trim() {
        "" => DEFAULT_OAUTH_CLIENT_ID.to_owned(),
        value => value.to_owned(),
    };

    info!(
        oauth_token_url = DEFAULT_OAUTH_TOKEN_URL,
        client_id, "token.toml 中没有 access token，开始使用 refresh token 获取"
    );
    let refreshed = exchange_refresh_token(client, &refresh_token, &client_id)
        .await
        .map_err(TokenResolutionError::RefreshFailed)?;
    let rotated_refresh_token_present = refreshed.refresh_token.is_some();
    config.access_token = refreshed.access_token.clone();
    if let Some(refresh_token) = refreshed.refresh_token {
        config.refresh_token = refresh_token;
    }
    let refreshed_account_id_present = refreshed.chatgpt_account_id.is_some();
    if let Some(chatgpt_account_id) = refreshed.chatgpt_account_id {
        config.chatgpt_account_id = chatgpt_account_id;
    }
    config.client_id = client_id;
    persist_token_config(token_file_path, config).map_err(|message| {
        TokenResolutionError::RefreshFailed(format!("OAuth 刷新成功，但{message}"))
    })?;
    info!(
        rotated_refresh_token_present,
        refreshed_account_id_present,
        effective_account_id_present = !config.chatgpt_account_id.trim().is_empty(),
        "OAuth 刷新结果已完整保存"
    );
    Ok((refreshed.access_token, "token-file-refresh-token"))
}

/// 按 Codex CLI 当前协议向 OpenAI OAuth token 端点发送 JSON 刷新请求。
async fn exchange_refresh_token(
    client: &Client,
    refresh_token: &str,
    client_id: &str,
) -> Result<RefreshedTokens, String> {
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
    let refresh_token = value
        .get("refresh_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let chatgpt_account_id = extract_refreshed_chatgpt_account_id(&value, access_token);
    info!(
        chatgpt_account_id_present = chatgpt_account_id.is_some(),
        "已通过 refresh token 获取 OAuth 身份"
    );
    Ok(RefreshedTokens {
        access_token: access_token.to_owned(),
        refresh_token,
        chatgpt_account_id,
    })
}

/// 按 OAuth 响应的显式字段、id_token、access_token 顺序提取 ChatGPT Account ID。
/// JWT payload 只用于取得上游路由提示，不参与本地认证决策；bearer token 的真实性仍由
/// OpenAI 上游验证，因此这里不重复实现 JWT 签名校验。
fn extract_refreshed_chatgpt_account_id(value: &Value, access_token: &str) -> Option<String> {
    value
        .get("chatgpt_account_id")
        .and_then(Value::as_str)
        .and_then(non_empty_owned)
        .or_else(|| {
            value
                .get("id_token")
                .and_then(Value::as_str)
                .and_then(extract_chatgpt_account_id_from_jwt)
        })
        .or_else(|| extract_chatgpt_account_id_from_jwt(access_token))
}

fn extract_chatgpt_account_id_from_jwt(token: &str) -> Option<String> {
    let payload = token.split('.').nth(1)?;
    let decoded = URL_SAFE_NO_PAD
        .decode(payload)
        .or_else(|_| URL_SAFE.decode(payload))
        .ok()?;
    let value: Value = serde_json::from_slice(&decoded).ok()?;
    value
        .get("chatgpt_account_id")
        .and_then(Value::as_str)
        .or_else(|| {
            value
                .get("https://api.openai.com/auth")
                .and_then(Value::as_object)
                .and_then(|auth| auth.get("chatgpt_account_id"))
                .and_then(Value::as_str)
        })
        .and_then(non_empty_owned)
}

fn non_empty_owned(value: &str) -> Option<String> {
    match value.trim() {
        "" => None,
        value => Some(value.to_owned()),
    }
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
    let mut trace = TraceRecorder::create(request_id).map_err(|error| {
        let trace_directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(TRACE_DIRECTORY_NAME);
        error!(
            request_id,
            trace_directory = %trace_directory.display(),
            error = %error,
            "创建请求调试记录文件失败"
        );
        ServiceError::internal(format!("创建请求调试记录文件失败: {error}"))
    })?;
    info!(
        request_id,
        trace_path = %trace.path().display(),
        "已创建请求调试记录文件"
    );
    write_trace_record(&mut trace, request_id, |trace| {
        trace.write_json_section("原始下游 Responses 请求体", &body)
    })?;

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

    let transformed_request = match transform_request(RequestTransformInput {
        account: AccountResource {
            access_token: state.access_token.to_string(),
            chatgpt_account_id: state.chatgpt_account_id.as_deref().map(str::to_owned),
            chatgpt_account_is_fedramp: false,
        },
        headers: request_headers_from_axum(&headers),
        body: body.to_vec(),
    }) {
        Ok(transformed) => transformed,
        Err(error) => {
            warn!(request_id, code = %error.code, message = %error.message, "请求插件函数拒绝请求");
            write_trace_record(&mut trace, request_id, |trace| {
                trace.write_error("请求插件转换", &error.code, &error.message)
            })?;
            return Err(ServiceError::bad_request(error.code, error.message));
        }
    };
    write_trace_record(&mut trace, request_id, |trace| {
        trace.write_json_section(
            "请求插件转换后实际发送至 Codex 的请求体",
            &transformed_request.body,
        )
    })?;

    let started_at = Instant::now();
    let upstream = match send_upstream(
        &state,
        state.responses_upstream_url.as_ref(),
        &transformed_request.headers,
        transformed_request.body,
    )
    .await
    {
        Ok(response) => response,
        Err(error) => {
            let error_detail = format_error_chain(&error);
            error!(request_id, error = %error_detail, "请求 Codex 上游失败");
            write_trace_record(&mut trace, request_id, |trace| {
                trace.write_error("请求 Codex 上游", "upstream_request_failed", &error_detail)
            })?;
            return Err(ServiceError::bad_gateway(
                "upstream_request_failed",
                error_detail,
            ));
        }
    };
    info!(
        request_id,
        upstream_status = upstream.status,
        upstream_bytes = upstream.body.len(),
        elapsed_ms = started_at.elapsed().as_millis(),
        "已收到 Codex 上游响应"
    );
    write_trace_record(&mut trace, request_id, |trace| {
        trace.write_upstream_response(upstream.status, &upstream.body)
    })?;

    // 与网关一致，按上游响应选择插槽；plugin-context 原样传递，服务不解析。
    let upstream_is_sse = upstream.headers.iter().any(|header| {
        header.name.eq_ignore_ascii_case("content-type")
            && std::str::from_utf8(&header.value)
                .ok()
                .and_then(|value| value.split(';').next())
                .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("text/event-stream"))
    });
    if !(200..300).contains(&upstream.status) || !upstream_is_sse {
        handle_buffered_response(
            request_id,
            upstream,
            transformed_request.plugin_context,
            &mut trace,
        )
    } else {
        handle_stream_response(
            request_id,
            upstream,
            transformed_request.plugin_context,
            &mut trace,
        )
    }
}

async fn send_upstream(
    state: &AppState,
    upstream_url: &str,
    headers: &[RequestHeader],
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
    plugin_context: Vec<u8>,
    trace: &mut TraceRecorder,
) -> Result<Response, ServiceError> {
    let upstream_was_sse = body_has_sse_framing(&response.body);
    let transformed = match transform_buffered_response(BufferedTransformInput {
        response: BufferedHttpResponse {
            status: response.status,
            headers: response
                .headers
                .into_iter()
                .map(|header| BufferedHeader {
                    name: header.name,
                    value: header.value,
                })
                .collect(),
            body: response.body,
        },
        plugin_context: Some(plugin_context),
    }) {
        Ok(transformed) => transformed,
        Err(error) => {
            error!(request_id, code = %error.code, message = %error.message, "缓冲响应插件函数执行失败");
            write_trace_record(trace, request_id, |trace| {
                trace.write_error("缓冲响应插件转换", &error.code, &error.message)
            })?;
            return Err(ServiceError::bad_gateway(error.code, error.message));
        }
    };
    log_buffered_effects(request_id, &transformed.effects);
    let BufferedDisposition::Respond(response) = transformed.disposition;
    if upstream_was_sse {
        write_trace_record(trace, request_id, |trace| {
            trace.write_json_section("流式转非流式后的最终 Responses 响应体", &response.body)
        })?;
    }
    if let Err(message) = validate_json_response(response.status, &response.body) {
        error!(request_id, validation_error = %message, "插件输出不是合法 Responses JSON");
        write_trace_record(trace, request_id, |trace| {
            trace.write_error(
                "最终非流式 Responses 校验",
                "invalid_responses_json",
                &message,
            )
        })?;
        return Err(ServiceError::bad_gateway("invalid_responses_json", message));
    }
    info!(
        request_id,
        status = response.status,
        "Responses JSON 格式校验通过"
    );
    match build_response(HttpResponse {
        status: response.status,
        headers: response
            .headers
            .into_iter()
            .map(|header| Header {
                name: header.name,
                value: header.value,
            })
            .collect(),
        body: response.body,
    }) {
        Ok(response) => {
            write_trace_record(trace, request_id, |trace| {
                trace.write_text_section(
                    "请求处理完成",
                    &format!(
                        "completed_at: {}\nresponse_mode: buffered",
                        trace_timestamp()
                    ),
                )
            })?;
            Ok(response)
        }
        Err(error) => {
            error!(request_id, error = %error.message, "构造下游非流式响应失败");
            write_trace_record(trace, request_id, |trace| {
                trace.write_error("构造下游非流式 HTTP 响应", &error.code, &error.message)
            })?;
            Err(error)
        }
    }
}

fn handle_stream_response(
    request_id: u64,
    response: HttpResponse,
    plugin_context: Vec<u8>,
    trace: &mut TraceRecorder,
) -> Result<Response, ServiceError> {
    let HttpResponse {
        status,
        headers,
        body,
    } = response;
    let mut transformer = StreamResponseTransformer::default();
    let head = match transformer.start(StreamStartInput {
        head: ResponseHead {
            status,
            headers: headers
                .into_iter()
                .map(|header| StreamHeader {
                    name: header.name,
                    value: header.value,
                })
                .collect(),
        },
        plugin_context: Some(plugin_context),
    }) {
        Ok(head) => head,
        Err(error) => {
            write_trace_record(trace, request_id, |trace| {
                trace.write_error("流式响应插件启动", &error.code, &error.message)
            })?;
            return Err(ServiceError::bad_gateway(error.code, error.message));
        }
    };
    let items = match split_sse_items(&body) {
        Ok(items) => items,
        Err(message) => {
            write_trace_record(trace, request_id, |trace| {
                trace.write_error("切分 Codex SSE 响应", "invalid_upstream_sse", &message)
            })?;
            return Err(ServiceError::bad_gateway("invalid_upstream_sse", message));
        }
    };
    let mut transformed_body = Vec::with_capacity(body.len());
    for item in items {
        let transformed = match transformer.transform_item(item) {
            Ok(transformed) => transformed,
            Err(error) => {
                error!(request_id, code = %error.code, message = %error.message, "流式响应插件函数执行失败");
                write_trace_record(trace, request_id, |trace| {
                    trace.write_error("流式响应插件 event 转换", &error.code, &error.message)
                })?;
                return Err(ServiceError::bad_gateway(error.code, error.message));
            }
        };
        log_stream_effects(request_id, &transformed.effects);
        if let Some(item) = transformed.item {
            transformed_body.extend_from_slice(&item);
        }
    }
    let finished = match transformer.finish() {
        Ok(finished) => finished,
        Err(error) => {
            write_trace_record(trace, request_id, |trace| {
                trace.write_error("流式响应插件结束", &error.code, &error.message)
            })?;
            return Err(ServiceError::bad_gateway(error.code, error.message));
        }
    };
    log_stream_effects(request_id, &finished.effects);
    for item in finished.items {
        transformed_body.extend_from_slice(&item);
    }

    if let Err(message) = validate_sse_response(&transformed_body) {
        error!(request_id, validation_error = %message, "插件输出不是合法 Responses SSE");
        write_trace_record(trace, request_id, |trace| {
            trace.write_error("最终流式 Responses 校验", "invalid_responses_sse", &message)
        })?;
        return Err(ServiceError::bad_gateway("invalid_responses_sse", message));
    }
    info!(
        request_id,
        event_stream_bytes = transformed_body.len(),
        "Responses SSE 格式校验通过"
    );
    match build_response(HttpResponse {
        status: head.status,
        headers: head
            .headers
            .into_iter()
            .map(|header| Header {
                name: header.name,
                value: header.value,
            })
            .collect(),
        body: transformed_body,
    }) {
        Ok(response) => {
            write_trace_record(trace, request_id, |trace| {
                trace.write_text_section(
                    "请求处理完成",
                    &format!("completed_at: {}\nresponse_mode: stream", trace_timestamp()),
                )
            })?;
            Ok(response)
        }
        Err(error) => {
            error!(request_id, error = %error.message, "构造下游流式响应失败");
            write_trace_record(trace, request_id, |trace| {
                trace.write_error("构造下游流式 HTTP 响应", &error.code, &error.message)
            })?;
            Err(error)
        }
    }
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

fn request_headers_from_axum(headers: &HeaderMap) -> Vec<RequestHeader> {
    headers
        .iter()
        .map(|(name, value)| RequestHeader {
            name: name.as_str().to_owned(),
            value: value.as_bytes().to_vec(),
        })
        .collect()
}

fn to_reqwest_headers(headers: &[RequestHeader]) -> reqwest::header::HeaderMap {
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

fn log_buffered_effects(request_id: u64, effects: &BufferedEffects) {
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
}

fn log_stream_effects(request_id: u64, effects: &StreamEffects) {
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
