use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};

use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use bytes::Bytes;
use chrono::{DateTime, Utc};
use serde::de::DeserializeOwned;
use serde_json::Value;
use uuid::Uuid;
use wasmtime::component::{Component, Linker};
use wasmtime::{Config, Engine, Store, StoreLimits, StoreLimitsBuilder};

use crate::{
    err::{AppError, AppResult},
    provider::{
        claude::model::ClaudeAccountRequestContext,
        gpt::model::GptAccountRequestContext,
        protocol::{MAX_SSE_ITEM_BYTES, UpstreamFeedback},
    },
    request_event::TokenUsage,
};

use super::{
    PROVIDER_CLAUDE, PROVIDER_GPT,
    model::{PluginArtifactBinding, PluginBinding, PluginSlot},
};

/// 请求插件和 buffered 响应插件处理完整 JSON，继续使用较小的线性内存边界。
/// stream 插件一次会接收一个完整 SSE item；图片结果等合法 Responses item 可能远大于
/// 该值，因此 stream session 使用单独的更高上限，不能与另外两个插槽共用此常量。
const PLUGIN_MEMORY_LIMIT_BYTES: usize = 64 * 1024 * 1024;
/// 500 MiB item 通过 canonical ABI 进入 guest 后，原始字节、JSON DOM 与必要的重写结果
/// 会在调用期间并存。2 GiB 为这些瞬时副本留出空间，同时仍给单个 stream session 设置
/// 明确边界，避免异常组件无限增长线性内存。
const STREAM_PLUGIN_MEMORY_LIMIT_BYTES: usize = 2 * 1024 * 1024 * 1024;
const MAX_OUTPUT_BODY_BYTES: usize = 64 * 1024 * 1024;
const _: () = assert!(STREAM_PLUGIN_MEMORY_LIMIT_BYTES > MAX_SSE_ITEM_BYTES);
/// opaque context 只应保存跨请求/响应阶段必需的最小映射。限制独立于 body，避免插件借此
/// 绕过响应体上限，或因异常输出让单次 attempt 长时间占用大块宿主内存。
const MAX_PLUGIN_CONTEXT_BYTES: usize = 64 * 1024;
const MAX_HEADER_VALUES: usize = 256;
const MAX_HEADER_BYTES: usize = 128 * 1024;
const MAX_PLUGIN_TEXT_BYTES: usize = 1024;

mod gpt_request_bindings {
    wasmtime::component::bindgen!({
        path: "wit/request-transformer.wit",
        world: "gpt-request-transformer",
    });
}

mod claude_request_bindings {
    wasmtime::component::bindgen!({
        path: "wit/request-transformer.wit",
        world: "claude-request-transformer",
    });
}

mod gpt_buffered_bindings {
    wasmtime::component::bindgen!({
        path: "wit/buffered-response-transformer.wit",
        world: "gpt-buffered-response-transformer",
    });
}

mod claude_buffered_bindings {
    wasmtime::component::bindgen!({
        path: "wit/buffered-response-transformer.wit",
        world: "claude-buffered-response-transformer",
    });
}

mod gpt_stream_bindings {
    wasmtime::component::bindgen!({
        path: "wit/stream-response-transformer.wit",
        world: "gpt-stream-response-transformer",
    });
}

mod claude_stream_bindings {
    wasmtime::component::bindgen!({
        path: "wit/stream-response-transformer.wit",
        world: "claude-stream-response-transformer",
    });
}

pub enum RequestPluginInput {
    Gpt {
        headers: HeaderMap,
        body: Bytes,
        account: GptPluginAccount,
    },
    Claude {
        headers: HeaderMap,
        body: Bytes,
        account: ClaudePluginAccount,
    },
}

pub struct GptPluginAccount {
    access_token: String,
    context: GptAccountRequestContext,
}

pub struct ClaudePluginAccount {
    access_token: String,
    context: ClaudeAccountRequestContext,
}

impl RequestPluginInput {
    /// 调度层只会在 OAuth account attempt 调用该构造函数，因此这里只接收插件需要的
    /// 最小账号投影，不再接收同时包含官方 API Key 的通用上游资源类型。响应插件完全
    /// 接管 wire 响应，但永远看不到 access token、官方 Key、资源 ID、分组或 runtime 状态。
    pub fn from_account(
        provider: &str,
        resource_id: Uuid,
        access_token: &str,
        request_context: &Value,
        headers: HeaderMap,
        body: Bytes,
    ) -> AppResult<Self> {
        match provider {
            PROVIDER_GPT => Ok(Self::Gpt {
                headers,
                body,
                account: GptPluginAccount {
                    access_token: access_token.trim().to_owned(),
                    context: parse_account_context(provider, resource_id, request_context)?,
                },
            }),
            PROVIDER_CLAUDE => Ok(Self::Claude {
                headers,
                body,
                account: ClaudePluginAccount {
                    access_token: access_token.trim().to_owned(),
                    context: parse_account_context(provider, resource_id, request_context)?,
                },
            }),
            provider => Err(AppError::Plugin {
                message: format!("无法为未知 Provider 构造请求插件输入: {provider}"),
            }),
        }
    }
}

pub struct RequestPluginOutput {
    pub headers: HeaderMap,
    pub body: Bytes,
    /// 请求插件声明的成功响应下游交付模式。它可以与实际上游传输协议不同，并必须和
    /// response context 一样按 attempt 传递，不能在收到响应后根据 Header 猜测。
    pub response_mode: PluginResponseMode,
    pub response_context: Option<Bytes>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginResponseMode {
    Stream,
    Buffered,
}

impl PluginResponseMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stream => "stream",
            Self::Buffered => "buffered",
        }
    }
}

#[derive(Clone)]
pub struct RawPluginResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Bytes,
}

/// buffered 响应与产生该响应的请求插件 context 必须作为一个 attempt-local 输入传递，
/// 禁止通过 request_id 全局查找，避免重试或并发请求串用上下文。
pub struct BufferedPluginInput {
    pub response: RawPluginResponse,
    pub request_context: Option<Bytes>,
}

pub struct PluginEffects {
    pub feedback: Option<UpstreamFeedback>,
    pub usage: Option<TokenUsage>,
    pub failure: Option<PluginStreamFailure>,
}

pub struct PluginStreamFailure {
    pub kind: String,
    pub message: String,
}

pub enum BufferedPluginDisposition {
    Respond(RawPluginResponse),
    Retry {
        exclude_current_resource: bool,
        reason: String,
    },
}

pub struct BufferedPluginOutput {
    pub disposition: BufferedPluginDisposition,
    pub effects: PluginEffects,
}

pub struct StreamPluginItemOutput {
    /// 保留与 effects 对应的完整原始 item，仅供宿主在插件报告 failure 时写 tracing。
    pub upstream_item: Bytes,
    pub item: Option<Bytes>,
    pub effects: PluginEffects,
}

pub struct StreamPluginFinishOutput {
    pub items: Vec<Bytes>,
    pub effects: PluginEffects,
}

/// `start` 同时创建本次 SSE 响应的有状态 Component session，并返回插件决定的最终
/// 下游响应头。宿主只保留 HTTP 结构与凭证防泄漏校验，不再透传原始上游 head。
pub struct StreamPluginStartOutput {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub session: StreamPluginSession,
}

pub(crate) struct StoreData {
    limits: StoreLimits,
}

#[derive(Clone)]
pub struct PluginRuntime {
    inner: Arc<PluginRuntimeInner>,
}

struct PluginRuntimeInner {
    engine: Engine,
    compiled: RwLock<HashMap<Uuid, CachedComponent>>,
}

struct CachedComponent {
    wasm_sha256: String,
    component: Arc<Component>,
}

impl PluginRuntime {
    pub fn new() -> AppResult<Self> {
        let mut config = Config::new();
        config.wasm_component_model(true);
        let engine = Engine::new(&config).map_err(|source| AppError::Startup {
            message: format!("初始化 Wasmtime 插件套件引擎失败: {source}"),
        })?;
        tracing::info!(
            plugin_memory_limit_bytes = PLUGIN_MEMORY_LIMIT_BYTES,
            stream_plugin_memory_limit_bytes = STREAM_PLUGIN_MEMORY_LIMIT_BYTES,
            max_sse_item_bytes = MAX_SSE_ITEM_BYTES,
            max_plugin_context_bytes = MAX_PLUGIN_CONTEXT_BYTES,
            "WASM 插件套件运行时已初始化"
        );
        Ok(Self {
            inner: Arc::new(PluginRuntimeInner {
                engine,
                compiled: RwLock::new(HashMap::new()),
            }),
        })
    }

    /// 发布和冷加载共用这个严格 ABI 入口。Linker 不注册 WASI、网络、文件或 host callback；
    /// feedback/usage 只能作为一次成功调用的返回值交给宿主，插件无法产生半完成副作用。
    pub fn compile(
        &self,
        provider: &str,
        slot: PluginSlot,
        wasm_bytes: &[u8],
    ) -> AppResult<Arc<Component>> {
        let component = Arc::new(
            Component::from_binary(&self.inner.engine, wasm_bytes).map_err(|source| {
                AppError::BadRequest {
                    message: format!("WASM Component 编译或校验失败: {source}"),
                }
            })?,
        );
        let mut store = self.new_store(PLUGIN_MEMORY_LIMIT_BYTES)?;
        let linker = Linker::<StoreData>::new(&self.inner.engine);
        match (provider, slot) {
            (PROVIDER_GPT, PluginSlot::Request) => {
                gpt_request_bindings::GptRequestTransformer::instantiate(
                    &mut store, &component, &linker,
                )
                .map_err(|source| abi_error(provider, slot, source))?;
            }
            (PROVIDER_CLAUDE, PluginSlot::Request) => {
                claude_request_bindings::ClaudeRequestTransformer::instantiate(
                    &mut store, &component, &linker,
                )
                .map_err(|source| abi_error(provider, slot, source))?;
            }
            (PROVIDER_GPT, PluginSlot::BufferedResponse) => {
                gpt_buffered_bindings::GptBufferedResponseTransformer::instantiate(
                    &mut store, &component, &linker,
                )
                .map_err(|source| abi_error(provider, slot, source))?;
            }
            (PROVIDER_CLAUDE, PluginSlot::BufferedResponse) => {
                claude_buffered_bindings::ClaudeBufferedResponseTransformer::instantiate(
                    &mut store, &component, &linker,
                )
                .map_err(|source| abi_error(provider, slot, source))?;
            }
            (PROVIDER_GPT, PluginSlot::StreamResponse) => {
                gpt_stream_bindings::GptStreamResponseTransformer::instantiate(
                    &mut store, &component, &linker,
                )
                .map_err(|source| abi_error(provider, slot, source))?;
            }
            (PROVIDER_CLAUDE, PluginSlot::StreamResponse) => {
                claude_stream_bindings::ClaudeStreamResponseTransformer::instantiate(
                    &mut store, &component, &linker,
                )
                .map_err(|source| abi_error(provider, slot, source))?;
            }
            (provider, _) => {
                return Err(AppError::BadRequest {
                    message: format!("插件套件 provider 不受支持: {provider}"),
                });
            }
        }
        Ok(component)
    }

    pub fn cache_component(
        &self,
        artifact_id: Uuid,
        wasm_sha256: String,
        component: Arc<Component>,
    ) -> AppResult<()> {
        let mut cache = self.inner.compiled.write().map_err(|_| AppError::Plugin {
            message: "插件编译缓存写锁已损坏".to_owned(),
        })?;
        cache.insert(
            artifact_id,
            CachedComponent {
                wasm_sha256,
                component,
            },
        );
        Ok(())
    }

    pub fn cached_component(
        &self,
        artifact: &PluginArtifactBinding,
    ) -> AppResult<Option<Arc<Component>>> {
        let cache = self.inner.compiled.read().map_err(|_| AppError::Plugin {
            message: "插件编译缓存读锁已损坏".to_owned(),
        })?;
        let Some(cached) = cache.get(&artifact.id) else {
            return Ok(None);
        };
        if cached.wasm_sha256 != artifact.wasm_sha256 {
            return Err(AppError::Plugin {
                message: format!("插件缓存摘要与 artifact 快照不一致: {}", artifact.id),
            });
        }
        Ok(Some(cached.component.clone()))
    }

    pub fn execute_request(
        &self,
        binding: &PluginBinding,
        component: &Component,
        input: RequestPluginInput,
    ) -> AppResult<RequestPluginOutput> {
        match (binding.provider.as_str(), input) {
            (
                PROVIDER_GPT,
                RequestPluginInput::Gpt {
                    headers,
                    body,
                    account,
                },
            ) => self.execute_gpt_request(component, headers, body, account),
            (
                PROVIDER_CLAUDE,
                RequestPluginInput::Claude {
                    headers,
                    body,
                    account,
                },
            ) => self.execute_claude_request(component, headers, body, account),
            (provider, _) => Err(AppError::Plugin {
                message: format!("请求插件输入与套件 Provider 不匹配: {provider}"),
            }),
        }
    }

    pub fn execute_buffered(
        &self,
        binding: &PluginBinding,
        component: &Component,
        input: BufferedPluginInput,
    ) -> AppResult<BufferedPluginOutput> {
        match binding.provider.as_str() {
            PROVIDER_GPT => self.execute_gpt_buffered(component, input),
            PROVIDER_CLAUDE => self.execute_claude_buffered(component, input),
            provider => Err(AppError::Plugin {
                message: format!("buffered 响应插件 Provider 不受支持: {provider}"),
            }),
        }
    }

    pub fn start_stream(
        &self,
        binding: &PluginBinding,
        component: &Component,
        status: StatusCode,
        headers: &HeaderMap,
        request_context: Option<Bytes>,
    ) -> AppResult<StreamPluginStartOutput> {
        match binding.provider.as_str() {
            PROVIDER_GPT => self.start_gpt_stream(component, status, headers, request_context),
            PROVIDER_CLAUDE => {
                self.start_claude_stream(component, status, headers, request_context)
            }
            provider => Err(AppError::Plugin {
                message: format!("stream 响应插件 Provider 不受支持: {provider}"),
            }),
        }
    }

    fn execute_gpt_request(
        &self,
        component: &Component,
        headers: HeaderMap,
        body: Bytes,
        account: GptPluginAccount,
    ) -> AppResult<RequestPluginOutput> {
        use gpt_request_bindings::aestus::request_transformer::{common_types, gpt_types};
        let account = gpt_types::AccountResource {
            access_token: account.access_token,
            chatgpt_account_id: account.context.chatgpt_account_id,
            chatgpt_account_is_fedramp: account.context.chatgpt_account_is_fedramp,
        };
        let input = gpt_types::TransformInput {
            account,
            headers: headers_to_wit::<common_types::Header>(&headers)?,
            body: body.to_vec(),
        };
        let mut store = self.new_store(PLUGIN_MEMORY_LIMIT_BYTES)?;
        let linker = Linker::<StoreData>::new(&self.inner.engine);
        let bindings = gpt_request_bindings::GptRequestTransformer::instantiate(
            &mut store, component, &linker,
        )
        .map_err(plugin_runtime_error)?;
        let output = bindings
            .call_transform(&mut store, &input)
            .map_err(plugin_runtime_error)?
            .map_err(|error| declared_error(&error.code, &error.message))?;
        let response_mode = match output.response_mode {
            common_types::ResponseMode::Stream => PluginResponseMode::Stream,
            common_types::ResponseMode::Buffered => PluginResponseMode::Buffered,
        };
        request_output_from_parts(
            output
                .headers
                .into_iter()
                .map(|header| (header.name, header.value)),
            output.body,
            response_mode,
            output.response_context,
        )
    }

    fn execute_claude_request(
        &self,
        component: &Component,
        headers: HeaderMap,
        body: Bytes,
        account: ClaudePluginAccount,
    ) -> AppResult<RequestPluginOutput> {
        use claude_request_bindings::aestus::request_transformer::{claude_types, common_types};
        let account = claude_types::AccountResource {
            access_token: account.access_token,
            account_uuid: account.context.account_uuid.to_string(),
        };
        let input = claude_types::TransformInput {
            account,
            headers: headers_to_wit::<common_types::Header>(&headers)?,
            body: body.to_vec(),
        };
        let mut store = self.new_store(PLUGIN_MEMORY_LIMIT_BYTES)?;
        let linker = Linker::<StoreData>::new(&self.inner.engine);
        let bindings = claude_request_bindings::ClaudeRequestTransformer::instantiate(
            &mut store, component, &linker,
        )
        .map_err(plugin_runtime_error)?;
        let output = bindings
            .call_transform(&mut store, &input)
            .map_err(plugin_runtime_error)?
            .map_err(|error| declared_error(&error.code, &error.message))?;
        let response_mode = match output.response_mode {
            common_types::ResponseMode::Stream => PluginResponseMode::Stream,
            common_types::ResponseMode::Buffered => PluginResponseMode::Buffered,
        };
        request_output_from_parts(
            output
                .headers
                .into_iter()
                .map(|header| (header.name, header.value)),
            output.body,
            response_mode,
            output.response_context,
        )
    }

    fn execute_gpt_buffered(
        &self,
        component: &Component,
        input: BufferedPluginInput,
    ) -> AppResult<BufferedPluginOutput> {
        use gpt_buffered_bindings::aestus::buffered_response_transformer::common_types;
        let input = common_types::TransformInput {
            response: common_types::Response {
                status: input.response.status.as_u16(),
                headers: headers_to_wit::<common_types::Header>(&input.response.headers)?,
                body: input.response.body.to_vec(),
            },
            request_context: input.request_context.map(|context| context.to_vec()),
        };
        let mut store = self.new_store(PLUGIN_MEMORY_LIMIT_BYTES)?;
        let linker = Linker::<StoreData>::new(&self.inner.engine);
        let bindings = gpt_buffered_bindings::GptBufferedResponseTransformer::instantiate(
            &mut store, component, &linker,
        )
        .map_err(plugin_runtime_error)?;
        let output = bindings
            .call_transform(&mut store, &input)
            .map_err(plugin_runtime_error)?
            .map_err(|error| declared_error(&error.code, &error.message))?;
        buffered_output_from_gpt(output)
    }

    fn execute_claude_buffered(
        &self,
        component: &Component,
        input: BufferedPluginInput,
    ) -> AppResult<BufferedPluginOutput> {
        use claude_buffered_bindings::aestus::buffered_response_transformer::common_types;
        let input = common_types::TransformInput {
            response: common_types::Response {
                status: input.response.status.as_u16(),
                headers: headers_to_wit::<common_types::Header>(&input.response.headers)?,
                body: input.response.body.to_vec(),
            },
            request_context: input.request_context.map(|context| context.to_vec()),
        };
        let mut store = self.new_store(PLUGIN_MEMORY_LIMIT_BYTES)?;
        let linker = Linker::<StoreData>::new(&self.inner.engine);
        let bindings = claude_buffered_bindings::ClaudeBufferedResponseTransformer::instantiate(
            &mut store, component, &linker,
        )
        .map_err(plugin_runtime_error)?;
        let output = bindings
            .call_transform(&mut store, &input)
            .map_err(plugin_runtime_error)?
            .map_err(|error| declared_error(&error.code, &error.message))?;
        buffered_output_from_claude(output)
    }

    fn start_gpt_stream(
        &self,
        component: &Component,
        status: StatusCode,
        headers: &HeaderMap,
        request_context: Option<Bytes>,
    ) -> AppResult<StreamPluginStartOutput> {
        use gpt_stream_bindings::aestus::stream_response_transformer::common_types;
        let mut store = self.new_store(STREAM_PLUGIN_MEMORY_LIMIT_BYTES)?;
        let linker = Linker::<StoreData>::new(&self.inner.engine);
        let bindings = gpt_stream_bindings::GptStreamResponseTransformer::instantiate(
            &mut store, component, &linker,
        )
        .map_err(plugin_runtime_error)?;
        let input = common_types::StartInput {
            head: common_types::ResponseHead {
                status: status.as_u16(),
                headers: headers_to_wit::<common_types::Header>(headers)?,
            },
            request_context: request_context.map(|context| context.to_vec()),
        };
        let output = bindings
            .call_start(&mut store, &input)
            .map_err(plugin_runtime_error)?
            .map_err(|error| declared_error(&error.code, &error.message))?;
        let (status, headers) = stream_response_head_from_parts(
            output.status,
            output
                .headers
                .into_iter()
                .map(|header| (header.name, header.value)),
        )?;
        Ok(StreamPluginStartOutput {
            status,
            headers,
            session: StreamPluginSession::Gpt { store, bindings },
        })
    }

    fn start_claude_stream(
        &self,
        component: &Component,
        status: StatusCode,
        headers: &HeaderMap,
        request_context: Option<Bytes>,
    ) -> AppResult<StreamPluginStartOutput> {
        use claude_stream_bindings::aestus::stream_response_transformer::common_types;
        let mut store = self.new_store(STREAM_PLUGIN_MEMORY_LIMIT_BYTES)?;
        let linker = Linker::<StoreData>::new(&self.inner.engine);
        let bindings = claude_stream_bindings::ClaudeStreamResponseTransformer::instantiate(
            &mut store, component, &linker,
        )
        .map_err(plugin_runtime_error)?;
        let input = common_types::StartInput {
            head: common_types::ResponseHead {
                status: status.as_u16(),
                headers: headers_to_wit::<common_types::Header>(headers)?,
            },
            request_context: request_context.map(|context| context.to_vec()),
        };
        let output = bindings
            .call_start(&mut store, &input)
            .map_err(plugin_runtime_error)?
            .map_err(|error| declared_error(&error.code, &error.message))?;
        let (status, headers) = stream_response_head_from_parts(
            output.status,
            output
                .headers
                .into_iter()
                .map(|header| (header.name, header.value)),
        )?;
        Ok(StreamPluginStartOutput {
            status,
            headers,
            session: StreamPluginSession::Claude { store, bindings },
        })
    }

    fn new_store(&self, memory_limit_bytes: usize) -> AppResult<Store<StoreData>> {
        let limits = StoreLimitsBuilder::new()
            .memory_size(memory_limit_bytes)
            .instances(1)
            .memories(1)
            .tables(4)
            .build();
        let mut store = Store::new(&self.inner.engine, StoreData { limits });
        store.limiter(|data| &mut data.limits);
        Ok(store)
    }
}

pub enum StreamPluginSession {
    Gpt {
        store: Store<StoreData>,
        bindings: gpt_stream_bindings::GptStreamResponseTransformer,
    },
    Claude {
        store: Store<StoreData>,
        bindings: claude_stream_bindings::ClaudeStreamResponseTransformer,
    },
}

impl StreamPluginSession {
    pub fn transform_item(&mut self, item: Bytes) -> AppResult<StreamPluginItemOutput> {
        if item.len() > MAX_SSE_ITEM_BYTES {
            return Err(AppError::Plugin {
                message: format!("原始 SSE item 超过插件上限: {} bytes", item.len()),
            });
        }
        let upstream_item = item.clone();
        match self {
            Self::Gpt { store, bindings } => {
                let output = bindings
                    .call_transform_item(store, item.as_ref())
                    .map_err(plugin_runtime_error)?
                    .map_err(|error| declared_error(&error.code, &error.message))?;
                stream_item_output_from_gpt(upstream_item, output)
            }
            Self::Claude { store, bindings } => {
                let output = bindings
                    .call_transform_item(store, item.as_ref())
                    .map_err(plugin_runtime_error)?
                    .map_err(|error| declared_error(&error.code, &error.message))?;
                stream_item_output_from_claude(upstream_item, output)
            }
        }
    }

    pub fn finish(&mut self) -> AppResult<StreamPluginFinishOutput> {
        match self {
            Self::Gpt { store, bindings } => {
                let output = bindings
                    .call_finish(store)
                    .map_err(plugin_runtime_error)?
                    .map_err(|error| declared_error(&error.code, &error.message))?;
                stream_finish_output_from_gpt(output)
            }
            Self::Claude { store, bindings } => {
                let output = bindings
                    .call_finish(store)
                    .map_err(plugin_runtime_error)?
                    .map_err(|error| declared_error(&error.code, &error.message))?;
                stream_finish_output_from_claude(output)
            }
        }
    }
}

fn parse_account_context<T: DeserializeOwned>(
    provider: &str,
    resource_id: Uuid,
    request_context: &Value,
) -> AppResult<T> {
    serde_json::from_value(request_context.clone()).map_err(|source| AppError::BadRequest {
        message: format!(
            "OAuth account 请求上下文无法解析: provider={provider}, resource_id={resource_id}, error={source}"
        ),
    })
}

trait WitHeader: Sized {
    fn new(name: String, value: Vec<u8>) -> Self;
}

macro_rules! impl_wit_header {
    ($type:path) => {
        impl WitHeader for $type {
            fn new(name: String, value: Vec<u8>) -> Self {
                Self { name, value }
            }
        }
    };
}

impl_wit_header!(gpt_request_bindings::aestus::request_transformer::common_types::Header);
impl_wit_header!(claude_request_bindings::aestus::request_transformer::common_types::Header);
impl_wit_header!(
    gpt_buffered_bindings::aestus::buffered_response_transformer::common_types::Header
);
impl_wit_header!(
    claude_buffered_bindings::aestus::buffered_response_transformer::common_types::Header
);
impl_wit_header!(gpt_stream_bindings::aestus::stream_response_transformer::common_types::Header);
impl_wit_header!(claude_stream_bindings::aestus::stream_response_transformer::common_types::Header);

fn headers_to_wit<H: WitHeader>(headers: &HeaderMap) -> AppResult<Vec<H>> {
    if headers.len() > MAX_HEADER_VALUES {
        return Err(AppError::Plugin {
            message: format!("原始 Header 值数量超过插件上限: {}", headers.len()),
        });
    }
    let mut total_bytes = 0usize;
    headers
        .iter()
        .map(|(name, value)| {
            total_bytes = total_bytes
                .saturating_add(name.as_str().len())
                .saturating_add(value.as_bytes().len());
            if total_bytes > MAX_HEADER_BYTES {
                return Err(AppError::Plugin {
                    message: format!("原始 Header 总大小超过插件上限: {MAX_HEADER_BYTES}"),
                });
            }
            Ok(H::new(name.as_str().to_owned(), value.as_bytes().to_vec()))
        })
        .collect()
}

fn request_output_from_parts(
    headers: impl Iterator<Item = (String, Vec<u8>)>,
    body: Vec<u8>,
    response_mode: PluginResponseMode,
    response_context: Option<Vec<u8>>,
) -> AppResult<RequestPluginOutput> {
    if body.len() > MAX_OUTPUT_BODY_BYTES {
        return Err(AppError::Plugin {
            message: format!("请求插件输出 Body 超过限制: {} bytes", body.len()),
        });
    }
    let mut headers = parse_headers(headers)?;
    headers.remove(header::HOST);
    headers.remove(header::CONTENT_LENGTH);
    let response_context = validate_plugin_context(response_context)?;
    Ok(RequestPluginOutput {
        headers,
        body: Bytes::from(body),
        response_mode,
        response_context,
    })
}

fn validate_plugin_context(context: Option<Vec<u8>>) -> AppResult<Option<Bytes>> {
    let Some(context) = context else {
        return Ok(None);
    };
    if context.is_empty() {
        return Err(AppError::Plugin {
            message: "请求插件输出的 response-context 不能为空；无上下文时应返回 none".to_owned(),
        });
    }
    if context.len() > MAX_PLUGIN_CONTEXT_BYTES {
        return Err(AppError::Plugin {
            message: format!(
                "请求插件输出的 response-context 超过限制: {} bytes，最大 {} bytes",
                context.len(),
                MAX_PLUGIN_CONTEXT_BYTES
            ),
        });
    }
    Ok(Some(Bytes::from(context)))
}

fn response_from_parts(
    status: u16,
    headers: impl Iterator<Item = (String, Vec<u8>)>,
    body: Vec<u8>,
) -> AppResult<RawPluginResponse> {
    let status = StatusCode::from_u16(status).map_err(|source| AppError::Plugin {
        message: format!("响应插件输出非法 HTTP status: {source}"),
    })?;
    if status.is_informational() {
        return Err(AppError::Plugin {
            message: format!("响应插件不能输出 1xx 最终响应: {status}"),
        });
    }
    if body.len() > MAX_OUTPUT_BODY_BYTES {
        return Err(AppError::Plugin {
            message: format!("响应插件输出 Body 超过限制: {} bytes", body.len()),
        });
    }
    let mut headers = parse_headers(headers)?;
    sanitize_downstream_headers(&mut headers);
    Ok(RawPluginResponse {
        status,
        headers,
        body: Bytes::from(body),
    })
}

fn stream_response_head_from_parts(
    status: u16,
    headers: impl Iterator<Item = (String, Vec<u8>)>,
) -> AppResult<(StatusCode, HeaderMap)> {
    let status = StatusCode::from_u16(status).map_err(|source| AppError::Plugin {
        message: format!("stream 响应插件输出非法 HTTP status: {source}"),
    })?;
    if status.is_informational()
        || matches!(
            status,
            StatusCode::NO_CONTENT | StatusCode::RESET_CONTENT | StatusCode::NOT_MODIFIED
        )
    {
        return Err(AppError::Plugin {
            message: format!("stream 响应插件输出的 HTTP status 不允许携带 SSE body: {status}"),
        });
    }
    let mut headers = parse_headers(headers)?;
    sanitize_downstream_headers(&mut headers);
    // Connection 是 hop-by-hop header，不能信任插件或上游提供的任意值，所以先走统一
    // 清理，再由宿主为最终 SSE HTTP/1.1 响应写入唯一的权威值。HTTP/2/3 发送层会按
    // 协议要求忽略该字段，不影响这些协议的流式响应。
    headers.insert(header::CONNECTION, HeaderValue::from_static("keep-alive"));
    Ok((status, headers))
}

fn parse_headers(headers: impl Iterator<Item = (String, Vec<u8>)>) -> AppResult<HeaderMap> {
    let mut output = HeaderMap::new();
    let mut total_bytes = 0usize;
    for (index, (name, value)) in headers.enumerate() {
        if index >= MAX_HEADER_VALUES {
            return Err(AppError::Plugin {
                message: format!("插件输出 Header 值数量超过限制: {MAX_HEADER_VALUES}"),
            });
        }
        total_bytes = total_bytes
            .saturating_add(name.len())
            .saturating_add(value.len());
        if total_bytes > MAX_HEADER_BYTES {
            return Err(AppError::Plugin {
                message: format!("插件输出 Header 总大小超过限制: {MAX_HEADER_BYTES}"),
            });
        }
        let name = HeaderName::from_bytes(name.as_bytes()).map_err(|source| AppError::Plugin {
            message: format!("插件输出非法 Header 名称: {source}"),
        })?;
        let mut value = HeaderValue::from_bytes(&value).map_err(|source| AppError::Plugin {
            message: format!("插件输出非法 Header 值: {source}"),
        })?;
        if name == header::AUTHORIZATION || name.as_str() == "x-api-key" {
            value.set_sensitive(true);
        }
        output.append(name, value);
    }
    Ok(output)
}

fn sanitize_downstream_headers(headers: &mut HeaderMap) {
    let connection_declared = headers
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .filter_map(|name| HeaderName::from_bytes(name.trim().as_bytes()).ok())
        .collect::<Vec<_>>();
    for name in connection_declared {
        headers.remove(name);
    }
    for name in [
        header::CONNECTION,
        header::TRANSFER_ENCODING,
        header::TE,
        header::TRAILER,
        header::UPGRADE,
        header::CONTENT_LENGTH,
        header::SET_COOKIE,
        header::AUTHORIZATION,
    ] {
        headers.remove(name);
    }
    headers.remove(header::HOST);
    headers.remove("x-api-key");
    headers.remove("keep-alive");
    headers.remove("proxy-authenticate");
    headers.remove("proxy-authorization");
}

fn validate_usage(usage: TokenUsage) -> AppResult<TokenUsage> {
    if [
        usage.input_tokens,
        usage.cached_input_tokens,
        usage.output_tokens,
        usage.reasoning_output_tokens,
        usage.total_tokens,
    ]
    .into_iter()
    .any(|value| value < 0)
    {
        return Err(AppError::Plugin {
            message: "插件 usage 不能包含负数".to_owned(),
        });
    }
    if usage.cached_input_tokens > usage.input_tokens {
        return Err(AppError::Plugin {
            message: "插件 cached_input_tokens 不能大于 input_tokens".to_owned(),
        });
    }
    if usage.reasoning_output_tokens > usage.output_tokens {
        return Err(AppError::Plugin {
            message: "插件 reasoning_output_tokens 不能大于 output_tokens".to_owned(),
        });
    }
    let expected_total = usage
        .input_tokens
        .checked_add(usage.output_tokens)
        .ok_or_else(|| AppError::Plugin {
            message: "插件 usage total 计算溢出".to_owned(),
        })?;
    if usage.total_tokens != expected_total {
        return Err(AppError::Plugin {
            message: format!(
                "插件 total_tokens 必须等于 input_tokens + output_tokens: expected={expected_total}, actual={}",
                usage.total_tokens
            ),
        });
    }
    Ok(usage)
}

fn feedback_from_parts(
    kind: FeedbackKind,
    reason: String,
    resets_at: Option<i64>,
) -> AppResult<UpstreamFeedback> {
    let reason = bounded_text(reason, "feedback.reason")?;
    let resets_at = resets_at.map(timestamp_from_seconds).transpose()?;
    Ok(match kind {
        FeedbackKind::Error => UpstreamFeedback::Error { reason },
        FeedbackKind::AuthenticationRejected => UpstreamFeedback::AuthenticationRejected { reason },
        FeedbackKind::RateLimited => UpstreamFeedback::RateLimited { resets_at, reason },
        FeedbackKind::QuotaExhausted => UpstreamFeedback::QuotaExhausted { resets_at, reason },
        FeedbackKind::TemporarilyUnavailable => UpstreamFeedback::TemporarilyUnavailable { reason },
        FeedbackKind::EntitlementMissing => UpstreamFeedback::EntitlementMissing { reason },
    })
}

enum FeedbackKind {
    Error,
    AuthenticationRejected,
    RateLimited,
    QuotaExhausted,
    TemporarilyUnavailable,
    EntitlementMissing,
}

fn timestamp_from_seconds(seconds: i64) -> AppResult<DateTime<Utc>> {
    DateTime::from_timestamp(seconds, 0).ok_or_else(|| AppError::Plugin {
        message: format!("插件 feedback reset 时间戳非法: {seconds}"),
    })
}

fn bounded_text(value: String, field: &str) -> AppResult<String> {
    if value.trim().is_empty() {
        return Err(AppError::Plugin {
            message: format!("插件 {field} 不能为空"),
        });
    }
    if value.len() > MAX_PLUGIN_TEXT_BYTES {
        return Err(AppError::Plugin {
            message: format!("插件 {field} 超过 {MAX_PLUGIN_TEXT_BYTES} 字节"),
        });
    }
    Ok(value)
}

fn validate_sse_item(bytes: Vec<u8>) -> AppResult<Bytes> {
    if bytes.is_empty() || bytes.len() > MAX_SSE_ITEM_BYTES {
        return Err(AppError::Plugin {
            message: format!("插件输出 SSE item 大小非法: {} bytes", bytes.len()),
        });
    }
    if !super::sse::is_exact_item(&bytes) {
        return Err(AppError::Plugin {
            message: "插件输出必须恰好包含一个带结束空行的完整 SSE item".to_owned(),
        });
    }
    Ok(Bytes::from(bytes))
}

fn abi_error(provider: &str, slot: PluginSlot, source: impl std::fmt::Display) -> AppError {
    AppError::BadRequest {
        message: format!(
            "组件不符合 {provider} {} ABI 或声明了未授权 import: {source}",
            slot.as_str()
        ),
    }
}

fn plugin_runtime_error(source: impl std::fmt::Display) -> AppError {
    AppError::Plugin {
        message: format!("WASM 运行时错误: {source}"),
    }
}

fn declared_error(code: &str, message: &str) -> AppError {
    AppError::Plugin {
        message: format!(
            "插件拒绝处理: code={}, message={}",
            truncate(code, MAX_PLUGIN_TEXT_BYTES),
            truncate(message, MAX_PLUGIN_TEXT_BYTES)
        ),
    }
}

fn truncate(value: &str, max_bytes: usize) -> &str {
    let mut end = value.len().min(max_bytes);
    while !value.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    &value[..end]
}

// bindgen 为不同 world 生成彼此独立的同形类型。下面保持显式转换，让新增 ABI 字段时
// 编译器强制所有 Provider/插槽同步处理，而不是通过无类型 JSON 悄悄丢失控制事实。

fn buffered_output_from_gpt(
    output: gpt_buffered_bindings::aestus::buffered_response_transformer::common_types::TransformOutput,
) -> AppResult<BufferedPluginOutput> {
    use gpt_buffered_bindings::aestus::buffered_response_transformer::common_types as wit;
    let disposition = match output.disposition {
        wit::BufferedDisposition::Respond(response) => {
            BufferedPluginDisposition::Respond(response_from_parts(
                response.status,
                response
                    .headers
                    .into_iter()
                    .map(|header| (header.name, header.value)),
                response.body,
            )?)
        }
        wit::BufferedDisposition::Retry(retry) => BufferedPluginDisposition::Retry {
            exclude_current_resource: retry.exclude_current_resource,
            reason: bounded_text(retry.reason, "retry.reason")?,
        },
    };
    let feedback = output
        .effects
        .feedback
        .map(gpt_buffered_feedback)
        .transpose()?;
    let usage = output
        .effects
        .usage
        .map(|usage| {
            validate_usage(TokenUsage {
                input_tokens: usage.input_tokens,
                cached_input_tokens: usage.cached_input_tokens,
                output_tokens: usage.output_tokens,
                reasoning_output_tokens: usage.reasoning_output_tokens,
                total_tokens: usage.total_tokens,
            })
        })
        .transpose()?;
    Ok(BufferedPluginOutput {
        disposition,
        effects: PluginEffects {
            feedback,
            usage,
            failure: None,
        },
    })
}

fn buffered_output_from_claude(
    output: claude_buffered_bindings::aestus::buffered_response_transformer::common_types::TransformOutput,
) -> AppResult<BufferedPluginOutput> {
    use claude_buffered_bindings::aestus::buffered_response_transformer::common_types as wit;
    let disposition = match output.disposition {
        wit::BufferedDisposition::Respond(response) => {
            BufferedPluginDisposition::Respond(response_from_parts(
                response.status,
                response
                    .headers
                    .into_iter()
                    .map(|header| (header.name, header.value)),
                response.body,
            )?)
        }
        wit::BufferedDisposition::Retry(retry) => BufferedPluginDisposition::Retry {
            exclude_current_resource: retry.exclude_current_resource,
            reason: bounded_text(retry.reason, "retry.reason")?,
        },
    };
    let feedback = output
        .effects
        .feedback
        .map(claude_buffered_feedback)
        .transpose()?;
    let usage = output
        .effects
        .usage
        .map(|usage| {
            validate_usage(TokenUsage {
                input_tokens: usage.input_tokens,
                cached_input_tokens: usage.cached_input_tokens,
                output_tokens: usage.output_tokens,
                reasoning_output_tokens: usage.reasoning_output_tokens,
                total_tokens: usage.total_tokens,
            })
        })
        .transpose()?;
    Ok(BufferedPluginOutput {
        disposition,
        effects: PluginEffects {
            feedback,
            usage,
            failure: None,
        },
    })
}

fn gpt_buffered_feedback(
    feedback: gpt_buffered_bindings::aestus::buffered_response_transformer::common_types::UpstreamFeedback,
) -> AppResult<UpstreamFeedback> {
    use gpt_buffered_bindings::aestus::buffered_response_transformer::common_types::UpstreamFeedback as F;
    match feedback {
        F::Error(reason) => feedback_from_parts(FeedbackKind::Error, reason, None),
        F::AuthenticationRejected(reason) => {
            feedback_from_parts(FeedbackKind::AuthenticationRejected, reason, None)
        }
        F::RateLimited(value) => feedback_from_parts(
            FeedbackKind::RateLimited,
            value.reason,
            value.resets_at_unix_seconds,
        ),
        F::QuotaExhausted(value) => feedback_from_parts(
            FeedbackKind::QuotaExhausted,
            value.reason,
            value.resets_at_unix_seconds,
        ),
        F::TemporarilyUnavailable(reason) => {
            feedback_from_parts(FeedbackKind::TemporarilyUnavailable, reason, None)
        }
        F::EntitlementMissing(reason) => {
            feedback_from_parts(FeedbackKind::EntitlementMissing, reason, None)
        }
    }
}

fn claude_buffered_feedback(
    feedback: claude_buffered_bindings::aestus::buffered_response_transformer::common_types::UpstreamFeedback,
) -> AppResult<UpstreamFeedback> {
    use claude_buffered_bindings::aestus::buffered_response_transformer::common_types::UpstreamFeedback as F;
    match feedback {
        F::Error(reason) => feedback_from_parts(FeedbackKind::Error, reason, None),
        F::AuthenticationRejected(reason) => {
            feedback_from_parts(FeedbackKind::AuthenticationRejected, reason, None)
        }
        F::RateLimited(value) => feedback_from_parts(
            FeedbackKind::RateLimited,
            value.reason,
            value.resets_at_unix_seconds,
        ),
        F::QuotaExhausted(value) => feedback_from_parts(
            FeedbackKind::QuotaExhausted,
            value.reason,
            value.resets_at_unix_seconds,
        ),
        F::TemporarilyUnavailable(reason) => {
            feedback_from_parts(FeedbackKind::TemporarilyUnavailable, reason, None)
        }
        F::EntitlementMissing(reason) => {
            feedback_from_parts(FeedbackKind::EntitlementMissing, reason, None)
        }
    }
}

fn stream_item_output_from_gpt(
    upstream_item: Bytes,
    output: gpt_stream_bindings::aestus::stream_response_transformer::common_types::ItemOutput,
) -> AppResult<StreamPluginItemOutput> {
    Ok(StreamPluginItemOutput {
        upstream_item,
        item: output.item.map(validate_sse_item).transpose()?,
        effects: gpt_stream_effects(output.effects)?,
    })
}

fn stream_item_output_from_claude(
    upstream_item: Bytes,
    output: claude_stream_bindings::aestus::stream_response_transformer::common_types::ItemOutput,
) -> AppResult<StreamPluginItemOutput> {
    Ok(StreamPluginItemOutput {
        upstream_item,
        item: output.item.map(validate_sse_item).transpose()?,
        effects: claude_stream_effects(output.effects)?,
    })
}

fn stream_finish_output_from_gpt(
    output: gpt_stream_bindings::aestus::stream_response_transformer::common_types::FinishOutput,
) -> AppResult<StreamPluginFinishOutput> {
    Ok(StreamPluginFinishOutput {
        items: output
            .items
            .into_iter()
            .map(validate_sse_item)
            .collect::<AppResult<_>>()?,
        effects: gpt_stream_effects(output.effects)?,
    })
}

fn stream_finish_output_from_claude(
    output: claude_stream_bindings::aestus::stream_response_transformer::common_types::FinishOutput,
) -> AppResult<StreamPluginFinishOutput> {
    Ok(StreamPluginFinishOutput {
        items: output
            .items
            .into_iter()
            .map(validate_sse_item)
            .collect::<AppResult<_>>()?,
        effects: claude_stream_effects(output.effects)?,
    })
}

fn gpt_stream_effects(
    effects: gpt_stream_bindings::aestus::stream_response_transformer::common_types::ResponseEffects,
) -> AppResult<PluginEffects> {
    let feedback = effects.feedback.map(gpt_stream_feedback).transpose()?;
    let usage = effects
        .usage
        .map(|usage| {
            validate_usage(TokenUsage {
                input_tokens: usage.input_tokens,
                cached_input_tokens: usage.cached_input_tokens,
                output_tokens: usage.output_tokens,
                reasoning_output_tokens: usage.reasoning_output_tokens,
                total_tokens: usage.total_tokens,
            })
        })
        .transpose()?;
    let failure = effects
        .failure
        .map(|failure| -> AppResult<_> {
            Ok(PluginStreamFailure {
                kind: bounded_text(failure.kind, "failure.kind")?,
                message: bounded_text(failure.message, "failure.message")?,
            })
        })
        .transpose()?;
    Ok(PluginEffects {
        feedback,
        usage,
        failure,
    })
}

fn claude_stream_effects(
    effects: claude_stream_bindings::aestus::stream_response_transformer::common_types::ResponseEffects,
) -> AppResult<PluginEffects> {
    let feedback = effects.feedback.map(claude_stream_feedback).transpose()?;
    let usage = effects
        .usage
        .map(|usage| {
            validate_usage(TokenUsage {
                input_tokens: usage.input_tokens,
                cached_input_tokens: usage.cached_input_tokens,
                output_tokens: usage.output_tokens,
                reasoning_output_tokens: usage.reasoning_output_tokens,
                total_tokens: usage.total_tokens,
            })
        })
        .transpose()?;
    let failure = effects
        .failure
        .map(|failure| -> AppResult<_> {
            Ok(PluginStreamFailure {
                kind: bounded_text(failure.kind, "failure.kind")?,
                message: bounded_text(failure.message, "failure.message")?,
            })
        })
        .transpose()?;
    Ok(PluginEffects {
        feedback,
        usage,
        failure,
    })
}

fn gpt_stream_feedback(
    feedback: gpt_stream_bindings::aestus::stream_response_transformer::common_types::UpstreamFeedback,
) -> AppResult<UpstreamFeedback> {
    use gpt_stream_bindings::aestus::stream_response_transformer::common_types::UpstreamFeedback as F;
    match feedback {
        F::Error(reason) => feedback_from_parts(FeedbackKind::Error, reason, None),
        F::AuthenticationRejected(reason) => {
            feedback_from_parts(FeedbackKind::AuthenticationRejected, reason, None)
        }
        F::RateLimited(value) => feedback_from_parts(
            FeedbackKind::RateLimited,
            value.reason,
            value.resets_at_unix_seconds,
        ),
        F::QuotaExhausted(value) => feedback_from_parts(
            FeedbackKind::QuotaExhausted,
            value.reason,
            value.resets_at_unix_seconds,
        ),
        F::TemporarilyUnavailable(reason) => {
            feedback_from_parts(FeedbackKind::TemporarilyUnavailable, reason, None)
        }
        F::EntitlementMissing(reason) => {
            feedback_from_parts(FeedbackKind::EntitlementMissing, reason, None)
        }
    }
}

fn claude_stream_feedback(
    feedback: claude_stream_bindings::aestus::stream_response_transformer::common_types::UpstreamFeedback,
) -> AppResult<UpstreamFeedback> {
    use claude_stream_bindings::aestus::stream_response_transformer::common_types::UpstreamFeedback as F;
    match feedback {
        F::Error(reason) => feedback_from_parts(FeedbackKind::Error, reason, None),
        F::AuthenticationRejected(reason) => {
            feedback_from_parts(FeedbackKind::AuthenticationRejected, reason, None)
        }
        F::RateLimited(value) => feedback_from_parts(
            FeedbackKind::RateLimited,
            value.reason,
            value.resets_at_unix_seconds,
        ),
        F::QuotaExhausted(value) => feedback_from_parts(
            FeedbackKind::QuotaExhausted,
            value.reason,
            value.resets_at_unix_seconds,
        ),
        F::TemporarilyUnavailable(reason) => {
            feedback_from_parts(FeedbackKind::TemporarilyUnavailable, reason, None)
        }
        F::EntitlementMissing(reason) => {
            feedback_from_parts(FeedbackKind::EntitlementMissing, reason, None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_context_rejects_empty_and_oversized_values() {
        assert!(validate_plugin_context(None).unwrap().is_none());
        assert!(validate_plugin_context(Some(Vec::new())).is_err());
        assert!(validate_plugin_context(Some(vec![0; MAX_PLUGIN_CONTEXT_BYTES + 1])).is_err());

        let context = validate_plugin_context(Some(vec![1, 2, 3]))
            .unwrap()
            .unwrap();
        assert_eq!(context.as_ref(), &[1, 2, 3]);
    }

    #[test]
    fn stream_response_head_restores_authoritative_keep_alive_after_sanitizing() {
        let (_, headers) = stream_response_head_from_parts(
            StatusCode::OK.as_u16(),
            vec![
                ("connection".to_owned(), b"x-upstream".to_vec()),
                ("x-upstream".to_owned(), b"must-be-removed".to_vec()),
                ("content-type".to_owned(), b"text/event-stream".to_vec()),
            ]
            .into_iter(),
        )
        .unwrap();

        assert_eq!(
            headers.get(header::CONNECTION),
            Some(&HeaderValue::from_static("keep-alive"))
        );
        assert!(!headers.contains_key("x-upstream"));
    }
}
