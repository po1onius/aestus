# GPT OAuth 标准 Responses 插件套件

本目录提供一套可直接上传到网关 Dashboard 的 GPT 插件，用来让三个插件插槽共同接管
一次 `/v1/responses` 请求。插件只实现 GPT OAuth Account 到 ChatGPT Codex Responses
上游的协议适配，不包含 `/chat/completions` 兼容、WebSocket v2、心跳、重试或计费策略。

Codex OAuth 上游固定使用 SSE。宿主按上游 Content-Type 选择响应插槽；SSE 成功响应
进入流式插件，逐个处理完整 SSE item。buffered 插件保留完整 SSE 转 JSON 的转换能力，
但上下文中的 `stream=false` 目前只被插件读取，不会触发宿主切换插槽或自动聚合。

三个组件分别是：

| Dashboard 插槽 | 构建产物 | 职责 |
| --- | --- | --- |
| 请求插件 | `target/gpt-codex-request.component.wasm` | 生成最终上游请求 body/header，并携带插件私有上下文 |
| 非流式响应插件 | `target/gpt-codex-buffered-response.component.wasm` | 处理完整上游 HTTP 响应或把完整 SSE 转为 JSON |
| 流式响应插件 | `target/gpt-codex-stream-response.component.wasm` | 处理响应头及每个完整 SSE item |

## 函数调用接口

三个插件分别拥有自己的完整转换实现，不依赖其他插件的业务类型或业务函数。每个 crate
公开不依赖 WASM/WIT 的 Rust 入口，可由普通 Rust 程序直接调用：

- `gpt-codex-request-plugin::transform_request`：改造 OAuth 请求 header/body，并返回插件私有上下文；
- `gpt-codex-buffered-response-plugin::transform_buffered_response`：处理非流式 JSON、错误响应
  以及完整 SSE 到 JSON 的转换；
- `gpt-codex-stream-response-plugin::StreamResponseTransformer`：处理一条流式响应的完整生命周期。

`gpt-codex-plugin-utils` 只提供通用 SSE framing 解析、切分和重渲染工具，不包含请求字段、
响应字段、header、usage、maintenance、重试或聚合规则。buffered 与 stream 即使当前存在
相同响应规则，也各自在自己的 crate 内实现，后续修改不会隐式改变另一个插件。三个 WASM
Component 只负责各自 WIT 类型映射，因此 Rust 函数入口和上传到 Dashboard 的组件仍使用
同一份插件内业务实现。

## 请求转换

请求插件会执行以下操作：

- 请求体必须是 JSON object，并且包含非空字符串 `model`；
- 保存调用方原始 `stream`。请求输出的 `plugin-context` 是 `list<u8>`，内容为 UTF-8
  JSON，例如 `{"stream":true}`。只有调用方原始 `stream` 为布尔 `true` 时，JSON 中的
  `stream` 才为 `true`，否则为 `false`；
- 发给 Codex 上游的 body 无论下游模式如何，都固定为 `store=false`、`stream=true`；
- 拒绝非空 `previous_response_id`。HTTP `/v1/responses` 不支持 Responses WebSocket v2 的
  连接态续链语义；
- 删除 `prompt=null`，并拒绝非空的顶层 `prompt`。非空值用于引用标准 Responses API 的
  可复用 Prompt 模板，但 ChatGPT Codex OAuth 上游不支持；请改用 `instructions` 和
  `input`；
- `model` 作为不透明的上游路由标识原样透传；插件不解析 alias、版本或 effort 后缀，也不
  使用 model 推导任何其他请求字段；
- 删除 Codex OAuth 上游不支持的字段：`max_output_tokens`、`temperature`、`top_p`、
  `frequency_penalty`、`presence_penalty`、`user`、`metadata`、
  `prompt_cache_retention`、`safety_identifier` 和 `stream_options`；
- `reasoning` 和 `include` 完全保持调用方原值；插件不归一化 `reasoning.effort`，也不会因
  `reasoning` 非空而自动添加 `reasoning.encrypted_content`；
- 将 `service_tier=fast` 归一化为 `priority`；`text` 完全保持调用方原值，插件不再针对
  `text.verbosity` 做模型判断或删除；
- `instructions` 缺失或为空时，注入固定的内置 Codex base prompt，不根据 model 选择；
- 将 input 中的 `role=system` 原位改为 `developer`，不会重复复制到顶层 `instructions`；
- 将字符串 `input` 转为 user message 数组；空字符串转为空数组；
- 所有 input item 的 `id` 以及 `item_reference` 完全保持调用方原值，不再判断续链输入或
  清理 message、工具 item 和 reasoning item 的标识；
- 工具调用与工具输出的 `call_id` 完全保持调用方原值，不添加 `fc_*` 前缀，也不对超长
  ID 做哈希；Codex 上游自行校验最多 64 个字符的限制；
- `input_image`、图片 URL、base64 payload 及 image generation tool 的所有字段完全保持
  调用方原值。

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

三个 ABI 共用 `plugin-context = list<u8>` 类型。它是插件私有的不透明字节，宿主只限制
容量并按 attempt 原样转交，不解析 JSON，不要求任何字段，也不从中提取响应模式。其他
插件可以约定完全不同的内容。未执行请求插件时，响应插件输入中的 `plugin-context` 为
`none`；执行过请求插件并返回空字节时则为 `some([])`。

本目录的 GPT Codex 套件自行约定使用 UTF-8 JSON object，并以布尔 `stream` 保存调用方
原始交付模式；这只是本套件的内部协议，不属于宿主或 WIT 的字段契约。

本套件的两个响应插件各自在业务入口解析 JSON 的 `stream`；非法 JSON、缺失或非布尔
`stream` 返回 `invalid_plugin_context`，额外字段不会被拒绝。上下文为 `none` 时解析结果
为 `None`。该字段目前只被读取，不用于限制插槽、切换响应模式或补齐 Response 字段。

这是对旧 record ABI 的破坏性修改；宿主升级后需要重新编译并上传三个插件组件。

请求插件主动返回的 `transform-error` 表示调用方请求不受支持或结构非法。宿主不会发送
上游 HTTP 请求，也不会调用 buffered/stream 响应插件，而是使用插件公开的 `code/message`
生成 Provider 原生格式的 HTTP 400。WASM trap、资源越界、ABI 调用失败或非法插件输出仍
属于插件执行故障，宿主会返回脱敏的 HTTP 502，不能通过声明错误伪装成调用方错误。

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

缓冲响应插件用于上游非 SSE 的成功响应，以及所有非 2xx HTTP 响应。

如果上游 body 已经是 JSON，插件只提取 effects 并执行通用响应字段转换，不校验它是否为
标准 Response object；非 2xx JSON 或非 JSON 错误响应仍保持上游状态和 body。如果成功响应
具有 SSE framing，则按以下顺序转换为一个 JSON：

1. 仅采用唯一的 completed、done、incomplete、failed 或 cancelled 终止对象作为最终
   Response 外壳，不从请求上下文或 `response.created/in_progress` 快照补齐字段；
2. 终止对象的 `output` 缺失或为空时，使用原始 `response.output_item.done.item`；全部事件
   提供 `output_index` 时按索引排序并要求从零连续，Codex 全部省略索引时才按到达顺序保留；
3. 不使用文本或工具 delta 臆造 output，不生成 item id/status/annotations，也不补充
   object、status、时间、error、incomplete_details、usage 或 usage 明细；
4. 插件不校验最终 Response 外壳、output item、usage 或状态是否符合 OpenAI Schema；这些
   校验由下游负责。非法 SSE JSON、缺少终止事件、多个终止事件、终止后的 done item 或索引
   冲突等导致转换本身无法确定的情况仍会返回插件错误。

当前 buffered disposition 只有 `respond`，插件自身不会要求宿主重试。

## 流式响应转换

流式响应插件按 `start -> transform-item -> finish` 运行：

- `start` 清理响应 header，并输出 SSE 所需 header；
- `transform-item` 接收一个包含结尾空行的完整 SSE item；无法解析的 item 会返回插件错误；
- 没有 JSON data 的合法 SSE item 原样透传；
- data 是 JSON 时先从原始事件提取 effects，再执行通用响应字段修正；
- 非终止事件不会上报 usage；同一条流中的 maintenance feedback 最多上报一次；
- `response.failed` 的 `error.code` 为 `usage_not_included` 或 `insufficient_quota` 时，先按
  原始事件上报 effects，再把整个下游 item 替换为固定的 `rate_limit_exceeded` client retry
  事件，与网关 GPT 原生 SSE observer 的行为一致；
- JSON 没有变化时保留原始 item 字节；发生变化时仅重新序列化 data，保留
  `event/id/retry/comment` 以及 CRLF/LF 风格；
- `finish` 不追加事件，只重置该实例的生命周期状态。

## 通用响应字段和 Header

非流式插件与流式插件分别实现并遵循以下字段修正规则：

- 除上述会替换为 client retry event 的两种账号额度失败外，`response.failed` 发给下游前
  删除 `instructions`、`output`、`usage`、`metadata`、
  `reasoning`、`tools`、`tool_choice`、`parallel_tool_calls`、`text`、`truncation`、
  `max_output_tokens` 和 `incomplete_details`，保留错误身份和消息；
- 流式原生 message、function/custom tool call 和 `image_generation_call` 字段保持不变；
  buffered 聚合也原样保留 done item，不改写 id、status、annotations、文本、工具参数、
  图片结果或其他模型输出语义字段。

响应 header 会删除 hop-by-hop、`Connection` 声明的动态连接级 header、失效的
`content-length`、上游 cookie、鉴权信息和所有 `x-codex` / `x-codex-*` 私有 header。
SSE 转 JSON 成功时输出
`application/json; charset=utf-8`；流式响应输出 `text/event-stream; charset=utf-8`、
`cache-control: no-cache` 和 `connection: keep-alive`。

## maintenance、usage 和日志

响应插件在改造原始 JSON/SSE item 之前提取 `effects`：

- HTTP `401`：`authentication-rejected`；
- HTTP `429` 且 `error.type=usage_limit_reached`：`quota-exhausted`；
- HTTP `429` 且 `error.type=usage_not_included`：`entitlement-missing`；
- SSE `response.failed` 且 `response.error.code=insufficient_quota`：`quota-exhausted`；
- SSE `response.failed` 且 `response.error.code=usage_not_included`：`entitlement-missing`；
- 其他 HTTP 状态、错误 type/code 或错误消息不产生 maintenance feedback；
- 流式 `response.failed`：额外返回 `stream-failure`；policy/safety 等业务拒绝不会误报
  maintenance feedback。

buffered 插件完成上述 HTTP `429` feedback 提取后，会把 `usage_limit_reached` 和
`usage_not_included` 的下游错误统一改写为 `error.type=rate_limit_exceeded`、
`error.message=Rate limit reached`；HTTP 状态仍为 `429`，其他错误字段保持不变。

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

成功响应按上游 `Content-Type` 选择响应插槽：`text/event-stream` 选择 stream，其他
类型选择 buffered。401、429、5xx 等非成功响应固定交给 buffered 插件，以便完整解析
错误正文和产生 maintenance 回执。宿主不读取原始请求或插件上下文中的 `stream` 来选槽。
如果被选择的响应插槽未配置插件，宿主会回到 Provider 原生响应处理流程。
已配置插件但引用失效时拒绝请求，不按空插槽处理。

宿主允许单个完整 SSE item 最大 500 MiB。stream Component 使用独立的 2 GiB 线性内存
边界，为 canonical ABI、JSON DOM 及必要重写产生的瞬时副本预留空间；请求和 buffered
响应 Component 使用 64 MiB 内存边界。插件输出 body 不能超过 64 MiB，`plugin-context`
不能超过 1 MiB。宿主在发送上游请求前仅校验上下文容量；超限视为插件执行故障，返回
脱敏 HTTP 502。上下文相关日志只记录是否存在及字节大小，不记录或解析内容。JSON 格式和
字段校验由本套件的响应插件执行，其他插件自行决定如何解释其上下文。

## 构建与上传

本机需要 Rust 的 `wasm32-unknown-unknown` target：

```bash
make check
make build
```

`make build` 会先生成三个 core WASM，再使用 workspace 内的 `gpt-plugin-componentize`
封装为 Dashboard 接受的 WebAssembly Component。

在 Dashboard 的“插件 → WASM 插件”中选择 Provider `GPT`，分别按请求、非流式响应、
流式响应插槽上传三个产物，每个插件有独立名称和备注。随后在“套件”中按插槽选择这些
已有插件创建组合；同一个插件可以被多个套件复用。套件至少选择一个插件，空插槽沿用
原生处理。套件创建后不能更换插件搭配，不再提供发布版本或历史版本选择。

平台管理员上传的插件和创建的套件为平台公共资源（`tenant_id = NULL`），对所有租户可用；
租户 owner 上传和创建的资源属于本租户。公共套件只能选择公共插件，租户套件可以混搭
公共插件和本租户插件。Provider 和插槽必须匹配，其他租户的私有插件不可引用。资源归属
由后端决定且不可更改，租户对公共资源只有查看和使用权限；普通用户只选择套件绑定 Key。

创建或修改网关 API Key 绑定时直接选择套件，Provider 必须与 Key 的分组一致。
插件仍只挂载在 GPT `/v1/responses` 和 Claude `/v1/messages`，并且只作用于 OAuth
Account attempt；其他接口及 Official API Key attempt 保持原来的原生处理逻辑。
调用方原始请求的 inspection、模型白名单授权、请求日志和 sticky 调度仍由宿主完成。

删除插件会在同一数据库事务内删除所有引用它的套件；公共插件的依赖扫描覆盖所有租户，
会同时删除引用它的公共套件及租户私有套件。单独删除套件不会删除插件。删除确认框显示
后端统计的受影响套件、租户和 Key 数量，实际删除以事务内取得的最新依赖为准。
关联 Key 保留原来的套件 ID，后续挂载端点请求因找不到有效套件返回 HTTP 401，
不会自动解除绑定或回退为无插件请求。Dashboard 显示绑定失效，用户可以重新选择套件，
也可以显式解除绑定。套件停用时同样拒绝挂载端点的新请求。

宿主在入口以短只读数据库快照校验套件、插件及插槽，取得已编译组件引用或冷缓存需要
的 WASM 字节，结束事务后完成编译。本次请求持有固定组合，后续响应阶段和内部重试
无需再次查库；与删除并发的准备操作以其快照为准，已准备好的请求可继续完成。
编译缓存位于各网关进程内存，不使用 Redis；删除插件或套件不会清理该缓存，当前也
没有 TTL 或容量淘汰。缓存保留到进程退出，但不能绕过入口数据库有效性校验。

共享插件上下文和三个 ABI 的唯一真源分别是：

- [`../srv/wit/plugin-types.wit`](../srv/wit/plugin-types.wit)：共享的
  `plugin-context = list<u8>` 类型；
- [`../srv/wit/request-transformer.wit`](../srv/wit/request-transformer.wit)
- [`../srv/wit/buffered-response-transformer.wit`](../srv/wit/buffered-response-transformer.wit)
- [`../srv/wit/stream-response-transformer.wit`](../srv/wit/stream-response-transformer.wit)

三个 ABI 都通过 `use` 引用同一个 `aestus:plugin-types/response-types`，构建会直接加载这些
WIT；共享类型或宿主 ABI 改动后，插件会在编译阶段发现不兼容。

用于真实调用 Codex 账号端点的验证服务已经独立到仓库根目录的
[`../codex-proto-test-server`](../codex-proto-test-server)，不属于本插件 workspace 或构建产物。
