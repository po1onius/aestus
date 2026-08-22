# GPT OAuth 标准 Responses 插件套件

本目录提供一套可直接上传到网关 Dashboard 的 GPT 插件，用来让三个插件插槽共同接管
一次 `/v1/responses` 请求。插件只实现 GPT OAuth Account 到 ChatGPT Codex Responses
上游的协议适配，不包含 `/chat/completions` 兼容、WebSocket v2、心跳、重试或计费策略。

Codex OAuth 上游固定使用 SSE。下游传入 `stream=false` 时，套件会完整收集上游事件流，
再转换为一个非流式 Responses JSON；下游传入 `stream=true` 时，则逐个处理完整 SSE item。

三个组件分别是：

| Dashboard 插槽 | 构建产物 | 职责 |
| --- | --- | --- |
| 请求插件 | `target/gpt-codex-request.component.wasm` | 生成最终上游请求 body/header，并声明下游响应模式 |
| 非流式响应插件 | `target/gpt-codex-buffered-response.component.wasm` | 处理完整上游 HTTP 响应或把完整 SSE 转为 JSON |
| 流式响应插件 | `target/gpt-codex-stream-response.component.wasm` | 处理响应头及每个完整 SSE item |

## 函数调用接口

实际转换能力集中在 `gpt-codex-plugin-common::functions`，不依赖 WASM 或 WIT 类型，可由
普通 Rust 程序直接调用：

- `transform_request`：改造 OAuth 请求 header/body，并返回下游响应模式；
- `transform_buffered_response`：处理非流式 JSON、错误响应以及完整 SSE 到 JSON 的转换；
- `StreamResponseTransformer::{start, transform_item, finish}`：处理一条流式响应的完整生命周期。

三个 WASM Component 只负责 WIT 类型映射，内部调用同一组函数，因此函数调用和上传到
Dashboard 的组件不会形成两套业务实现。

## 请求转换

请求插件会执行以下操作：

- 请求体必须是 JSON object，并且包含非空字符串 `model`；
- 保存调用方原始 `stream`。只有原始值为布尔 `true` 时输出 `response-mode=stream`，
  其他情况输出 `buffered`；
- 发给 Codex 上游的 body 无论下游模式如何，都固定为 `store=false`、`stream=true`；
- 拒绝非空 `previous_response_id`。HTTP `/v1/responses` 不支持 Responses WebSocket v2 的
  连接态续链语义；
- 删除 `prompt=null`，并拒绝非空的顶层 `prompt`。非空值用于引用标准 Responses API 的
  可复用 Prompt 模板，但 ChatGPT Codex OAuth 上游不支持；请改用 `instructions` 和
  `input`；
- 归一化已知 Codex model alias，并从 `-minimal/-none/-low/-medium/-high/-xhigh` 后缀
  推导 reasoning effort；未知 model 保持调用方原值；
- 删除 Codex OAuth 上游不支持的字段：`max_output_tokens`、`temperature`、`top_p`、
  `frequency_penalty`、`presence_penalty`、`user`、`metadata`、
  `prompt_cache_retention`、`safety_identifier` 和 `stream_options`；
- 将 `reasoning.effort=minimal` 改为 `none`；`include` 完全保持调用方原值，插件不会因
  `reasoning` 非空而自动添加 `reasoning.encrypted_content`；
- 将 `service_tier=fast` 归一化为 `priority`；`text` 完全保持调用方原值，插件不再针对
  `text.verbosity` 做模型判断或删除；
- `instructions` 缺失或为空时，按照调用方的原始 model 注入对应的内置 Codex base prompt；
- 将 input 中的 `role=system` 原位改为 `developer`，不会重复复制到顶层 `instructions`；
- 将字符串 `input` 转为 user message 数组；空字符串转为空数组；
- 所有 input item 的 `id` 以及 `item_reference` 完全保持调用方原值，不再判断续链输入或
  清理 message、工具 item 和 reasoning item 的标识；
- 工具调用的 `call_id` 统一为 `fc_*`；超过 64 字节时使用固定 domain separator 和
  SHA-256 规则压缩；
- 删除 payload 为空的 base64 `input_image`；如果 message 的 content 因此变空，则删除
  该 message。

`prompt_cache_key` 保持调用方原值，不参与 header 会话隔离。插件也不会生成或透传
`version`、`session_id`、`conversation_id`、device ID 或 installation ID。

### 当前不会执行的工具转换

当前实现保持原生 Responses `tools`、`tool_choice`、工具名称、namespace 和参数不变，不会：

- 把 legacy `functions/function_call` 转成 `tools/tool_choice`；
- 把嵌套的 Chat Completions function schema 拉平；
- 摊平 namespace 工具或改写工具名称；
- 改写图片工具的 `format/compression` 字段；
- 将 `role=tool` 转成 `function_call_output`；
- 对 Spark 模型注入图片能力说明。

请求插件当前始终返回空的 `response_context`，响应插件也不执行工具名称恢复或原始 model
回写。

## 请求 Header

请求插件使用白名单重建 Codex OAuth 上游 header。允许参与重建或从下游保留的 header 只有：

- `accept-language`、`content-type` 和用于识别受支持 Codex 客户端的 `user-agent`；
- `x-codex-beta-features`、`x-codex-turn-state`、`x-codex-turn-metadata`；
- `x-openai-internal-codex-responses-lite`。

插件随后固定或注入：

- `accept: text/event-stream`；
- `openai-beta: responses=experimental`；
- 根据 `user-agent` 重建配对的官方 Codex `user-agent` 和 `originator`；无法识别调用方
  身份时使用内置 fallback；调用方传入的 `originator` 不会成为最终值；
- 本次 OAuth access token；
- 可选的 ChatGPT account ID 和 FedRAMP 标记。

调用方网关凭证、Host、hop-by-hop header、任意伪造的账号 header 及其他非白名单 header
不会进入上游请求。

## 缓冲响应转换

缓冲响应插件用于下游 `stream=false` 的成功响应，以及所有非 2xx HTTP 响应。

如果上游 body 已经是 JSON，插件会直接提取 effects 并执行通用响应字段修正。如果成功
响应具有 SSE framing，则按以下顺序转换为一个 Responses JSON：

1. 优先提取首个 `response.completed`、`response.done` 或 `response.incomplete` 的
   `response` object；
2. 终止对象的 `output` 为空时，优先按到达顺序保留去重后的
   `response.output_item.done.item` 原始对象；
3. 完全没有 done item 时，才累计文本 delta、reasoning delta、function/custom tool item
   和参数 delta 重建 output；
4. `response.failed` 转为状态码 502、类型为 `upstream_error` 的 JSON 错误；
5. 没有可识别终止事件时不伪造成功响应，保留原始 SSE body。

当前 buffered disposition 只有 `respond`，插件自身不会要求宿主重试。

## 流式响应转换

流式响应插件按 `start -> transform-item -> finish` 运行：

- `start` 清理响应 header，并输出 SSE 所需 header；
- `transform-item` 接收一个包含结尾空行的完整 SSE item；无法解析的 item 会返回插件错误；
- 没有 JSON data 的合法 SSE item 原样透传；
- data 是 JSON 时先从原始事件提取 effects，再执行通用响应字段修正；
- 非终止事件不会上报 usage；同一条流中的 maintenance feedback 最多上报一次；
- JSON 没有变化时保留原始 item 字节；发生变化时仅重新序列化 data，保留
  `event/id/retry/comment` 以及 CRLF/LF 风格；
- `finish` 不追加事件，只重置该实例的生命周期状态。

## 通用响应字段和 Header

非流式 JSON 与每个 SSE data JSON 共用以下字段修正：

- 终止输出中的 `image_generation_call` 已有非空 result，但状态仍为
  `generating/in_progress` 时改为 `completed`；
- `response.failed` 发给下游前删除 `instructions`、`output`、`usage`、`metadata`、
  `reasoning`、`tools`、`tool_choice`、`parallel_tool_calls`、`text`、`truncation`、
  `max_output_tokens` 和 `incomplete_details`，保留错误身份和消息；
- 原生 message、function/custom tool call 的名称和参数保持不变。

响应 header 会删除 hop-by-hop、`Connection` 声明的动态连接级 header、失效的
`content-length`、上游 cookie 和鉴权信息。SSE 转 JSON 成功时输出
`application/json; charset=utf-8`；流式响应输出 `text/event-stream; charset=utf-8`、
`cache-control: no-cache` 和 `connection: keep-alive`。

## maintenance、usage 和日志

响应插件在改造原始 JSON/SSE item 之前提取 `effects`：

- `401` 或明确的鉴权错误：`authentication-rejected`；
- `usage_not_included/entitlement`：`entitlement-missing`；
- `insufficient_quota/quota_exhausted`：`quota-exhausted`；
- `429/rate_limit`：`rate-limited`；
- `5xx/overloaded`：`temporarily-unavailable`；
- 流式 `response.failed`：额外返回 `stream-failure`；policy/safety 等业务拒绝不会误报
  maintenance feedback。

usage 从最终 response 或终止 SSE event 的 `usage` 读取。`cached_tokens` 和
`reasoning_tokens` 分别映射到 ABI 明细字段，`total_tokens` 按 input + output 重新计算。
流式 usage 只在终止 item 上报。

WASM 组件不访问数据库、缓存、文件系统或网络，也没有日志 import。宿主负责校验 effects、
应用 maintenance、记录 usage，并打印插件调用和错误日志。组件不会把 token 或原始响应写入
外部日志。

## 宿主执行规则和资源边界

插件只作用于 OAuth Account attempt。宿主调度到 Official API Key 时会同时跳过请求、
buffered 响应和 stream 响应三个插槽，执行 Provider 原生代理流程；混合资源分组会在每次
调度后按实际资源类型决定。

成功响应优先使用请求插件声明的 `response-mode` 选择响应插槽，不依赖上游
`Content-Type`。401、429、5xx 等非成功响应固定交给 buffered 插件，以便完整解析错误正文
和产生 maintenance 回执。如果被选择的响应插槽没有上传组件，宿主会回到 Provider 原生
响应处理流程。

宿主允许单个完整 SSE item 最大 500 MiB。stream Component 使用独立的 2 GiB 线性内存
边界，为 canonical ABI、JSON DOM 及必要重写产生的瞬时副本预留空间；请求和 buffered
响应 Component 使用 64 MiB 内存边界。插件输出 body 不能超过 64 MiB。

## 构建与上传

本机需要 Rust 的 `wasm32-unknown-unknown` target：

```bash
make check
make build
```

`make build` 会先生成三个 core WASM，再使用 workspace 内的 `gpt-plugin-componentize`
封装为 Dashboard 接受的 WebAssembly Component。

在 Dashboard 的“插件”页面选择 Provider `GPT`，把三个产物上传到对应插槽后，以同一个
套件版本发布。创建网关 API Key 时选择该套件版本即可。调用方原始请求的 inspection、
模型白名单授权、请求日志和 sticky 调度仍在插件之前由宿主完成。

三个 ABI 的唯一真源分别是：

- [`../srv/wit/request-transformer.wit`](../srv/wit/request-transformer.wit)
- [`../srv/wit/buffered-response-transformer.wit`](../srv/wit/buffered-response-transformer.wit)
- [`../srv/wit/stream-response-transformer.wit`](../srv/wit/stream-response-transformer.wit)

构建会直接引用这些 WIT；宿主 ABI 改动后，插件会在编译阶段发现不兼容。

## 测试服务

`gpt-codex-plugin-test-server` 提供本地 `POST /v1/responses`，真实调用 Codex 账号端点并
依次执行请求、缓冲响应或流式响应插件函数。它会在返回前检查最终 JSON/SSE 是否符合
Responses API 的核心结构；校验失败返回 502 和 `plugin_test_error`。日志会记录请求大小、
模型、上游状态和具体校验错误，但不会记录 access token 或完整 prompt。

启动时必须传入 Codex 登录态的 access token：

```bash
cargo run -p gpt-codex-plugin-test-server -- \
  --access-token "$CODEX_ACCESS_TOKEN"
```

多账号 token 可以额外传入 `--chatgpt-account-id <ACCOUNT_ID>`，监听地址默认为
`127.0.0.1:3000`。非流式验证示例：

```bash
curl http://127.0.0.1:3000/v1/responses \
  -H 'content-type: application/json' \
  -d '{"model":"gpt-5.6-sol","input":"只回复 OK","stream":false}'
```

流式验证只需把请求中的 `stream` 改为 `true`。测试服务为了能在写出 HTTP 响应前完成整条
事件序列和终止事件校验，会先收集完整上游 SSE，再以 `text/event-stream` 返回；它用于验证
插件协议转换，不用于测量首 token 延迟。
