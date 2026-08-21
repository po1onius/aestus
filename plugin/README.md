# GPT OAuth 标准 Responses 插件套件

本目录提供一套可直接上传到网关 Dashboard 的 GPT 插件，用来验证三个插件插槽共同
接管一次 `/v1/responses` 请求。字段行为参考同级 `sub2api` 项目的 OpenAI OAuth 标准
Responses 路径，不包含 `/chat/completions` 兼容、心跳、重试或计费策略。OAuth 上游固定
使用 SSE；下游传入 `stream=false` 时，套件会在完整收集上游事件流后输出非流式 JSON。

三个组件分别是：

| Dashboard 插槽 | 构建产物 | 职责 |
| --- | --- | --- |
| 请求插件 | `target/gpt-codex-request.component.wasm` | 最终上游请求 body/header 及响应模式 |
| 非流式响应插件 | `target/gpt-codex-buffered-response.component.wasm` | 原始上游 HTTP 响应 |
| 流式响应插件 | `target/gpt-codex-stream-response.component.wasm` | 响应头及每个完整 SSE item |

## 请求字段

OAuth Account 分支会：

- 归一化已知 Codex model alias，并从 `-none/-low/-medium/-high/-xhigh` 后缀补齐 reasoning effort；未知 model 保持原值；
- 固定 `store=false`、`stream=true`，删除 ChatGPT internal API 不支持的采样、长度、用户、metadata、cache 和 `stream_options` 字段；
- 把 `reasoning.effort=minimal` 改为 `none`，reasoning 非空时补上 `include:["reasoning.encrypted_content"]`；
- 把 legacy `functions/function_call` 转成 `tools/tool_choice`，并把嵌套的 Chat Completions function schema 拉平为 Responses schema；
- 把 string input 转为 user message 数组，把 `role=tool` 转为 `function_call_output`；按续链信号过滤 `item_reference/id`，只修正工具 item 的 call ID，并用与 sub2api 相同的 SHA-256 规则压缩超长 ID；
- 把 system message 原位改为 developer，不重复复制到 `instructions`；instructions 为空时按原始 model 写入对应的 Codex base prompt；
- 归一化 `service_tier`，按模型能力过滤 `text.verbosity`；Spark 模型追加图片能力说明并删除其不支持的图片工具声明；
- 把图片工具的 `format/compression` 改为 `output_format/output_compression`，删除空 base64 input image；
- 把除内建 `image_gen` 外的 namespace 子工具摊平为 function；名称超过 64 字节时按 sub2api 规则截断并追加 SHA-256 短哈希，发现顶层或 namespace 间撞名时在发送前拒绝；
- 使用 Codex 原生 Responses header 白名单，固定 Responses beta 并配对官方 client identity，再注入本次 OAuth token、ChatGPT account ID 和 FedRAMP 标记；普通 HTTP/SSE 请求不额外生成 `version/session_id/conversation_id`。
- 在覆盖请求字段前保存调用方原始 `stream`：原始值为布尔 `true` 时输出
  `response-mode=stream`，否则输出 `buffered`。发给 ChatGPT 上游的 body 无论哪种下游
  模式仍固定为 `stream=true`。

请求插件会拒绝 HTTP `/v1/responses` 中的非空 `previous_response_id`；该字段只适用于
sub2api 的 Responses WebSocket v2。`prompt_cache_key` 保持下游原值，不参与 header 会话
隔离；本套件也不注入或透传 device/installation ID。

插件只作用于 OAuth Account attempt。宿主调度到 Official API Key 时会同时跳过请求、
非流式响应和流式响应三个插槽，完整执行 Provider 原生透明代理流程；即使同一 Provider
分组混合两类资源，也会在每次调度后按实际资源类型决定。请求 ABI 只包含 OAuth Account
凭证投影，无法表达 Official API Key；宿主也不会向 Component 暴露官方 Key 凭证。

## 响应字段

非流式 JSON body 和每个 SSE data JSON 共用以下逻辑：

- 终止输出中的 `image_generation_call` 已有 result 但仍为 `generating/in_progress` 时改为 `completed`；
- 使用请求插件通过宿主 opaque context 传来的精确映射，把摊平的 `function_call` 恢复为原始 `namespace + name`；普通 message 和原生双下划线工具名不会被猜测改写；
- `response.failed` 发给下游前删除 output、usage、instructions、tools 等冗余请求回显，但保留 error 身份和消息。

下游请求 `stream=false` 时，buffered 插件按 sub2api 默认 OAuth HTTP 路径把完整上游 SSE
转换成一个 Responses JSON：优先提取首个
`response.completed/response.done/response.incomplete` 的 `response` 对象；终止对象的
`output` 为空时，优先按到达顺序保留 `response.output_item.done` 的完整
原始 item，以免丢失 encrypted content、未来字段或未知 item 类型；完全没有 done item 时，
才累计文本 delta、reasoning delta、function item 与参数 delta 重建 output。终止事件中的
usage 在改造前通过 effects 上报；`response.failed` 转成 502 JSON 错误并保留 maintenance
判断。没有可识别终止事件时不伪造成功响应，保留原始 SSE body 作为协议异常兜底。

响应 header 会删除 hop-by-hop、失效的 `content-length`、上游 cookie 和鉴权信息。流式
header 统一输出 `text/event-stream; charset=utf-8`、`cache-control: no-cache` 和
`connection: keep-alive`；每个 SSE
item 的 `event/id/retry/comment` 及 CRLF/LF 风格保留，仅在 data 为 JSON 且确有字段变化时
重新序列化 data。SSE→JSON 成功时会把上游 `text/event-stream` 覆盖为
`application/json; charset=utf-8`。

宿主在成功响应上以请求插件输出的 `response-mode` 选择响应插槽，不依赖上游是否返回
`Content-Type: text/event-stream`；因此即使中间代理漏掉 Content-Type，流式请求仍由
stream 插件立即接管 header，非流式请求仍会完整收集 body 并执行 SSE→JSON。401、429、
5xx 等非成功响应固定交给 buffered 插件，便于完整解析错误正文和产生 maintenance 回执。

请求插件只在确实摊平 namespace 时输出版本化 JSON context。宿主把它与最终发送的上游
attempt 绑定，不解析内容，只在响应组件入口传入一次；切换资源、网络失败、空响应插槽或
请求结束都会丢弃。context 不包含 token、账号 ID 或完整请求体。原始 model 回写仍未实现。

## maintenance、usage 和日志

插件在改造下游 body/SSE item **之前**读取原始上游错误和 usage，并通过响应 ABI 的
`effects` 返回：

- `401` 或明确的鉴权错误：`authentication-rejected`；
- `insufficient_quota/quota_exhausted`：`quota-exhausted`；
- `429/rate_limit`：`rate-limited`；
- `usage_not_included/entitlement`：`entitlement-missing`；
- `5xx/overloaded`：`temporarily-unavailable`；
- SSE `response.failed`：额外返回 `stream-failure`，但 policy/safety 等业务拒绝不会误报 maintenance feedback。

usage 从最终 response 或终止 SSE event 的 `usage` 读取，`cached_tokens` 和
`reasoning_tokens` 分别映射到 ABI 明细字段，`total_tokens` 始终按 input + output 计算。
流式 usage 只在终止 item 的 effects 中上报，避免把兼容上游的 delta usage 当成累计快照。
宿主负责校验 effects、应用 maintenance、记录 usage 和打印插件调用/错误日志，不再解析
插件改造后的业务 body 来猜测这些事实。WASM ABI 没有日志 import，组件自身不会把 token
或原始响应写到外部日志。

宿主允许单个完整 SSE item 最大 500 MiB，与 sub2api 默认行上限一致；stream Component
使用独立的 2 GiB 线性内存边界，为 canonical ABI、JSON 解析及必要重写产生的瞬时副本
预留空间。请求和 buffered 响应 Component 仍保持 64 MiB 内存边界。

## 构建与上传

本机需要 Rust 的 `wasm32-unknown-unknown` target：

```bash
make check
make build
```

在 Dashboard 的“插件”页面选择 Provider `GPT`，把三个产物上传到对应插槽后，以
同一个套件版本发布。创建网关 API Key 时选择该套件版本即可。调用方原始请求的 inspection、
模型白名单授权、请求日志和 sticky 调度仍在插件之前由宿主完成。

三个 ABI 的唯一真源分别是：

- [`../srv/wit/request-transformer.wit`](../srv/wit/request-transformer.wit)
- [`../srv/wit/buffered-response-transformer.wit`](../srv/wit/buffered-response-transformer.wit)
- [`../srv/wit/stream-response-transformer.wit`](../srv/wit/stream-response-transformer.wit)

构建会直接引用这些 WIT；宿主 ABI 改动后，插件会在编译阶段发现不兼容。
