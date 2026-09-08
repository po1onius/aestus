# Aestus
<p>
  <img src="web/src/assets/token-gateway-logo.svg" alt="Aestus Logo" width="128" />
</p>
AI 模型网关:统一管理上游账号资源,对外提供标准 API

## 功能

- 统一 API:对外提供 OpenAI / Anthropic 风格接口
- 图片 API:提供 OpenAI 风格 `/v1/images/generations` 和 `/v1/images/edits`
- 搜索 API:代理 Codex standalone search `/v1/alpha/search`
- 资源池:自动调度与维护多个上游账号、Api Key
- 插件:内置codex -> responses兼容转换插件
- SaaS 租户:平台管理员分发明文租户码；首位注册者成为租户 owner，后续注册者成为普通用户
- 面板:平台管理员管理租户及公共插件、套件；租户 owner 管理本租户用户、网关 Api Key、账号、分组、插件和额度
- 网站首页: `/` 提供账号管理能力、三大使用痛点、标准协议与 WASM 插件特点，通过控制台入口登录或注册
- 用户并发:租户 owner 可设置并查看用户的每 Provider 并发，GPT 与 Claude 独立计数
- 分组授权:租户 owner 按 Provider 分组授权普通用户创建 Key，并可独立授予组内账号、
  官方 Key、额度、重置信息和请求覆盖的查看或操作权限
- 租户隔离:租户名直接作为 `tenant_id`，租户私有资源、请求日志和用量统计按该可读标识隔离；公共插件和套件跨租户可用

## 技术栈

| 类别 | 技术 |
| --- | --- |
| 后端 | Rust（axum） |
| 存储 | PostgreSQL / Redis / ClickHouse |
| 插件 | WebAssembly |
| 前端 | React |
| 部署 | Podman Compose |

## 架构特点

- 所有上游共用一条处理流水线,差异由适配层实现,易复用易扩展
- 请求智能调度资源,粘性会话保证cache hit,失败自动换资源重试
- 热插拔wasm插件,灵活修改请求与响应
- 请求全链路可追踪,各阶段事件写入时序日志,高性能统计
- 大请求缓存到文件,降低内存峰值
- 用户请求通过 Redis 可续租 lease 做原子并发准入，覆盖上传、上游处理和流式响应生命周期

## SaaS 租户与角色

- `platform_admin` 由启动配置初始化，管理租户生命周期、租户码及平台公共插件、套件，可只读查看各租户的上游账号与官方 API Key 资源，并可查看平台级请求日志和用量；请求日志支持按租户筛选。
- `tenant_owner` 是某个租户第一位使用有效租户码完成注册的用户。owner 可以管理本租户全部上游资源、分组、插件、普通用户和 API Key，并查看本租户所有用户的请求日志与用量。
- `tenant_user` 只能管理自己的 API Key，并查看自己的请求日志与用量。用户只能选择 owner
  已授权且启用的 Provider 分组；撤销分组授权后，该用户绑定此组的既有 Key 立即停止鉴权。
- owner 可以按分组额外授予普通用户查看组内 OAuth 账号或脱敏官方 API Key、查询 GPT
  额度、查看或使用额度重置、查看或修改请求覆盖的权限。资源可视是所有细粒度权限的前置；
  上游资源的导入、换组、启停、删除及 Provider 分组管理始终仅限 owner。
- 租户码以明文保存在 PostgreSQL。平台管理员可替换租户码；删除对应记录即撤销，已经注册的用户不受影响。
- owner 停用 Provider 分组时保留网关 Key，后续请求因分组停用返回 403；重新启用后可恢复调用。
- 删除 Provider 分组时，上游账号和官方 API Key 释放为未分组；网关 Key、其模型白名单、
  插件绑定及原分组 ID 均保留。后续请求因分组不存在返回 401。Dashboard 显示“分组已删除，
  Key 无效”，允许查看、复制和删除，不再允许编辑；需要调用时在有效分组下新建 Key。
  重新创建同名分组不会恢复旧 Key。分组模型白名单及用户分组授权、权限随分组删除。
- 每个租户最多一个 owner。租户停用后，该租户 Dashboard 登录态和所有网关 API Key 都不可用。

## 快速开始

```bash
# 本地快速启动
make dev
# 部署
cd deploy && cp .env.example .env
podman compose up -d
# 旧compose版本可能会启动失败（容器依赖问题）
# 可以分开启动
podman compose up -d clickhouse redis postgres
podman compose run --rm --no-deps migrate
podman compose up -d gateway
```

首页与控制台由 Rust 网关统一托管：`make dev` 会先构建 `web/dist`，再通过
`AESTUS_WEB_DIST_DIR=../web/dist` 提供静态资源，访问 `http://127.0.0.1:8080/` 即可打开首页。
Docker 镜像也已包含前端构建产物并配置托管目录，无需单独启动 Vite 或 Nginx。
首页更新后重新执行 `cd web && npm run build` 即可更新本地后端托管的页面。

## URL 与职责

公开首页使用 `/`，控制台页面统一使用 `/console` 前缀，控制台 API 统一使用
`/api/console` 前缀。页面按业务资源命名；平台管理员、租户 owner 和普通用户共用
相应 URL，可见功能、操作权限和数据范围由登录身份确定。

| 功能 | 页面 | 控制台 API |
| --- | --- | --- |
| 租户 | `/console/tenants` | `/api/console/tenants` |
| Provider 资源 | `/console/providers` | 见下文 |
| 插件与套件 | `/console/plugins` | `/api/console/plugins`、`/api/console/plugin-suites` |
| 用户 | `/console/users` | `/api/console/users` |
| 用量 | `/console/usage` | `/api/console/usage` |
| 网关 API Key | `/console/gateway-api-keys` | `/api/console/gateway-api-keys` |
| 请求日志与 Policy 日志 | `/console/request-logs` | `/api/console/request-logs`、`/api/console/policy-logs` |
| 审计日志 | `/console/audit-logs` | `/api/console/audit-logs` |

Provider 的 OAuth 账号和上游官方 Key 分别使用
`/api/console/providers/{provider}/accounts` 与
`/api/console/providers/{provider}/upstream-api-keys`，当前支持 `gpt` 和 `claude`。
跨 Provider 的分组管理和授权选项使用 `/api/console/provider-groups`。
网关 Key 的套件绑定使用 `PUT /api/console/gateway-api-keys/{id}/plugin-suite`。
登录、注册及当前身份查询使用 `/api/console/auth` 下的对应接口。

本次路径调整需要同步发布前后端，并更新控制台书签和直接调用控制台 API 的脚本；
不提供旧路径别名。对外模型协议继续使用 `/v1/...`，健康检查继续使用 `/healthz`。

## 插件与套件

平台管理员和租户 owner 在“插件”页面分别管理 WASM 插件和套件。两张表的 `tenant_id`
为空表示平台公共资源，非空表示租户私有资源；归属由后端根据登录身份确定，创建后固定。
平台管理员只管理公共资源，owner 只管理本租户资源，同时可查看和使用公共资源。普通用户
不能管理插件，但可为自己的网关 Key 选择公共套件或本租户套件。平台与租户资源可以同名，
界面通过“平台公共 / 本租户”来源区分。

插件按 Provider（GPT / Claude）及插槽（请求 / 非流式响应 / 流式响应）独立上传，保存名称、
WASM 和备注。公共套件只能引用公共插件；租户私有套件可以混搭公共插件和本租户插件，不能
引用其他租户的私有插件。套件至少选择一个插件，同一插件可被多个套件复用。套件创建后
组合固定，不提供版本管理。网关 API Key 可选择同 Provider 的启用套件，也可以不绑定插件。
公共资源不会绕过租户启停、用户权限、分组授权或模型白名单校验。

插件继续只挂载 GPT `/v1/responses` 和 Claude `/v1/messages`，在 OAuth Account attempt
执行；Official API Key attempt 和其他端点仍使用原生流程。空插槽沿用原生处理。

请求插件通过独立输出字段 `stream: bool` 声明下游交付模式：成功响应据此选择流式或
非流式响应插件，未执行请求插件时成功响应默认选择流式，非 2xx 响应固定选择非流式。
GPT Codex 套件在 `stream=false` 时由宿主完整读取上游 SSE，交给非流式插件聚合为 JSON。
`plugin-context` 继续作为插件私有字节原样传递。此请求 ABI 变更需要重新构建上传请求
插件；本套件的响应插件也需更新以移除旧上下文校验。

删除插件会同时删除所有引用它的套件；删除公共插件会跨租户删除受影响的私有套件。
删除确认前由后端统计受影响的套件、租户及 Key 数量，实际删除在事务中重新计算依赖。
删除套件本身不会删除独立插件。两种删除都保留 Key
的套件引用，挂载端点后续请求因套件不存在返回 401，Dashboard 显示“套件已删除，绑定
失效”；用户重新绑定或显式解除绑定后可恢复调用。套件停用也会拒绝挂载端点的新请求。

请求入口在一致的数据库快照中准备全部已配置组件，进行中请求的响应转换和内部重试
使用固定组合，不受后续删除影响。删除暂不清理进程内的 WASM 编译缓存；缓存没有 TTL
和容量淘汰，持续保留到进程退出，且不能绕过入口数据库校验。

本次表结构直接定义在初始化 migration 中，适用于空数据库。已执行旧版初始化 migration
的开发数据库需要重新初始化；单纯再次执行 migrate 不会修改已经记录为完成的 migration。
构建和上传示例见 [`plugin/README.md`](plugin/README.md)。

## 图片 API

`POST /v1/images/generations` 和 `POST /v1/images/edits` 进入与 Responses 相同的鉴权、
模型白名单、资源调度、重试、maintenance、额度和请求日志流程。当前公开的是所有 GPT
资源都能一致执行的 buffered `gpt-image-2` 子集：`model` 可以省略，省略时按
`gpt-image-2` 授权并向上游显式补齐；显式模型只接受 `gpt-image-2`。共同支持
`prompt`、`background`、`n`、`quality` 和 `size`，不支持 `stream=true`，其他参数会返回
请求错误而不会被静默忽略。

generations 接收 JSON；edits 接收 `multipart/form-data`，支持单个或多个 `image` /
`image[]` 文件字段，最多 16 张；每张必须是小于 50 MiB 的 PNG、JPEG 或 WebP。当前跨
Account 与 Official API Key 一致的编辑子集不包含 `mask`、`input_fidelity`、
`output_format` 等额外参数。

账号资源会请求 Codex `/images/generations`；官方 API Key 资源请求其 Base URL 下的相同
路径。图片编辑分别请求 `/images/edits`，两类资源都会在 resource override 后编码成
包含 data URL 的标准 Images JSON。两个图片上游路径可分别通过
`AESTUS_GPT_UPSTREAM_IMAGE_GENERATIONS_PATH` 和 `AESTUS_GPT_UPSTREAM_IMAGE_EDITS_PATH`
覆盖。

```bash
curl http://127.0.0.1:8080/v1/images/generations \
  -H 'authorization: Bearer <AESTUS_GATEWAY_KEY>' \
  -H 'content-type: application/json' \
  -d '{"model":"gpt-image-2","prompt":"一只坐在月球上的橘猫","size":"1024x1024"}'
```

```bash
curl http://127.0.0.1:8080/v1/images/edits \
  -H 'authorization: Bearer <AESTUS_GATEWAY_KEY>' \
  -F 'image=@./cat.png' \
  -F 'prompt=给猫加一顶红色帽子' \
  -F 'quality=high'
```

## 搜索 API

`POST /v1/alpha/search` 供 Codex standalone web search 使用，进入与 Responses 相同的
GPT 鉴权、模型白名单、资源调度和请求日志流程。请求 JSON 及上游 JSON 响应均直接透传，
网关只替换所选 Account 或 Official API Key 的凭证；仅对 Account 错误响应旁路识别下述 GPT 策略日志，
不据此改写响应或调度资源，也不记录 token usage。Codex model provider 的 Base URL 指向
Aestus 的 `/v1` 后，会自动请求该接口。

GPT 搜索上游路径默认是 `/alpha/search`，可通过 `AESTUS_GPT_UPSTREAM_SEARCH_PATH` 覆盖。

## 请求日志与用量

业务日志统一由 `srv/src/logs` 模块管理：`request` 负责请求事件聚合、ClickHouse 读写和
明细保留策略，`policy` 负责策略日志模型及 PostgreSQL 读写，`audit` 负责控制台审计，`runtime` 管理日志 writer
任务和聚合状态的超时回收。请求日志和用量查询共用 `logs/calendar` 的业务日边界计算。
`worker` 负责请求事件分发及后台消费者组装，额度扣减独立消费 usage 事件；核心请求链路
继续通过 `request/events` 发布事实，Provider 负责协议识别。控制台日志 API 负责鉴权、
确定租户与用户范围、校验参数，再调用日志模块查询。程序运行诊断继续由
`infra/logging` 的 tracing、stdout 和滚动文件承担。

后续业务日志按类型在 `logs` 下扩展自己的模型、写入和查询入口；与模型请求无关的日志
不经过请求事件聚合。请求日志与 Policy 日志保留各自的存储、分页和权限规则。

调度成功时通过非阻塞 `try_send` 发送上游账号或官方 API Key 的内部 UUID；worker 每次收到
资源事件就更新请求明细的 `resource_id`，收到结束事件后落库。未收到资源事件时该字段为空。
队列满时允许丢弃事件，日志处理不影响核心请求。请求日志详情可查看该 ID。

GPT OAuth 账号的原生响应处理分支会将以下策略错误作为独立日志发送给后台 worker：

- HTTP `400` 且 `error.code=cyber_policy`；
- HTTP `400` 或 `403` 且 `error.code=misalignment_policy_violation`；
- SSE `type=response.failed` 且 `response.error.code` 为 `cyber_policy`、
  `misalignment_policy_violation` 或 `bio_policy`。

HTTP 识别覆盖 Responses、图片生成、图片编辑和搜索；SSE 识别位于原生 Responses observer。
仅 Account 原生响应参与，绑定套件但所选响应插槽为空、回到原生处理时也参与；
Official API Key 和由响应插件接管的响应均不参与。这些 Account 策略错误不会触发内部重试。
adapter / observer 只返回识别结果，由通用 proxy / 流包装器发送日志事件；记录只增加日志
事实，既有响应、maintenance 和请求日志结果判定继续执行。

独立 PostgreSQL 表 `gpt_policy_violation_logs` 保存主键 `id`、鉴权时的 `tenant_id`、
`username` 快照、上游账号邮箱 `account_email`、实际观察时间 `occurred_at`（TIMESTAMPTZ）和
`error_code`，不使用外键。PostgreSQL writer 根据请求日志聚合的资源 ID，限定本租户和 GPT
Provider 查询账号 `specific.email`，保存查询时的邮箱快照；资源事件缺失、账号已删除或
未记录邮箱时保存 NULL。policy 表不保存资源 ID，ClickHouse 请求日志继续保存 `resource_id`。
策略事件仅携带 `request_id`、发生时间和错误码。日志模块的请求消费者在已有请求聚合中保存
首次命中；重复事件不覆盖错误码或时间，一个下游请求最多生成一条特殊日志。在请求结束
或沿用现有 24 小时超时回收流程收尾时，日志模块使用已有鉴权快照生成 PostgreSQL 写入任务。
策略字段仅存在于内存聚合中，不写入 ClickHouse 行或 `extra`，也不回查用户表。收尾时
缺少鉴权快照会记录警告并跳过特殊日志，普通请求日志仍按原有流程写入。
发布及 writer 投递均使用有界队列的 `try_send`；队列满允许丢弃，落库失败只记录诊断，
不阻塞或中断模型响应。租户 owner 可在 Dashboard 请求日志页切换“请求日志 / Policy 日志”，
按服务时区的自然日查看 Policy 日志的时间、用户名、账号邮箱和错误码，并使用游标翻页。
查询接口 `GET /api/console/policy-logs` 仅允许租户 owner 查看当前租户的数据，不允许客户端
指定租户；支持 `date`、`limit`（默认 100，最多 500）、成对的 `before_occurred_at` 与
`before_id` 游标。Policy 日志没有请求日志的 30 天查询限制。

策略日志表已归入初始化 migration `00000000000000_init`，用于无历史数据的数据库初始化：

```bash
cd deploy
podman compose run --rm --no-deps migrate
```

GPT 账号额度查询同时展示主 Codex 额度两个窗口内的“本窗口网关已记录 Token”。窗口起点由
上游重置时间减去原始窗口秒数得到，按账号汇总从起点（包含）到额度查询时间（不包含）之间
开始的请求，不按请求状态过滤。该数字只包含本网关已落库的用量；窗口时间无效、已过期或
起点超出日志保留期时不提供统计。额外额度项暂不推断模型或功能归属，人工重置后也仍按
上游返回的窗口时间计算。

ClickHouse 请求明细默认保留 30 天，可通过 `AESTUS_REQUEST_LOG_RETENTION_DAYS` 配置；服务启动时
会自动同步表 TTL，调小后超期明细将由 ClickHouse 后台异步删除。平台管理员全局时间线使用主排序键，租户 owner
和普通用户分别使用 `tenant_id`、`user_id` 轻量投影。请求写入时会按 `AESTUS_TIMEZONE` 计算固定的业务日，并通过增量物化
视图写入长期保留的日用量聚合表。Dashboard 的全历史总量、模型、API Key、用户分布和最近
365 天均只查询该聚合表。

`AESTUS_TIMEZONE` 必须是 IANA 时区，例如 `UTC` 或 `Asia/Shanghai`。它定义全部用户共用的业务日
边界；产生聚合数据后修改该值需要重建日聚合，不应将它当作普通的运行时开关。

## 公开接口限流

`api/rate_limit` 使用 `tower_governor` 按来源 IP 对以下接口分别限流。限流在正文解析、
数据库访问、密码校验和发送邮件之前执行；超限立即返回 HTTP 429，不排队等待。

| 接口 | 持续补充速率 | 突发容量 | 配置名称中的接口标识 |
| --- | --- | --- | --- |
| `POST /api/console/auth/login` | 30 次/分钟 | 10 次 | `LOGIN` |
| `POST /api/console/auth/register` | 6 次/分钟 | 2 次 | `REGISTER` |
| `POST /api/console/auth/register/email-code` | 6 次/分钟 | 2 次 | `EMAIL_CODE` |
| `GET /api/console/status` | 60 次/分钟 | 20 次 | `STATUS` |

使用 `AESTUS_PUBLIC_RATE_LIMIT_<接口标识>_PER_MINUTE` 和
`AESTUS_PUBLIC_RATE_LIMIT_<接口标识>_BURST` 分别配置速率和容量，均须为大于 0 的
整数；非法值或容量恢复时间超出库可表示范围的组合会阻止启动。例如 `AESTUS_PUBLIC_RATE_LIMIT_LOGIN_PER_MINUTE=30`
与 `AESTUS_PUBLIC_RATE_LIMIT_LOGIN_BURST=10` 表示初始允许连续请求 10 次，此后
每 2 秒补充一次额度，最多累计 10 次；不是按自然分钟重置的固定窗口。
Makefile、Compose 和 `deploy/.env.example` 均已包含这八项配置。

同一 IP 的不同接口独立计数，所有放行的请求都会消耗额度，包括后续参数校验、登录或
注册失败的请求；携带 Authorization 也不会跳过公开接口限流。Axum 自动支持的
`HEAD /api/console/status` 与 GET 共用额度。`/healthz`、静态资源、需凭证的
控制台接口及模型网关接口不进入此限流层。注册验证码原有的按邮箱发送冷却保持独立生效。

超限响应为 `{"error":{"code":"public_rate_limited","message":"请求过于频繁，请稍后再试"}}`，
并附带以秒为单位的 `Retry-After`。审计包在限流层外侧，因此 429 也会尝试写入审计；
限流拒绝发生在身份校验之前，身份字段为空，仅平台管理员可见。

来源 IP 由 `api/client_ip` 统一解析，审计和限流共用同一结果，遵循下述
`AESTUS_TRUSTED_PROXY_IPS` 规则。IPv4 映射 IPv6 地址规范化为 IPv4，避免同一地址
形成两个计数。缺少 TCP 连接信息时公开接口返回 500 并记录配置诊断，不跳过限流。

额度保存在进程内，不增加 Redis 或数据库访问；每 60 秒清理额度已完全恢复的 IP。
进程重启会重置额度，多副本各自计数，整体允许量会随副本数增加。

## 控制台请求审计

Axum 中间件覆盖 `/api/console` 的请求，包括登录、注册、查询、修改操作、鉴权拒绝、
未知路径和不支持的 HTTP 方法；审计查询本身也会生成一条记录。模型 API、静态页面和
健康检查不进入审计。`api/console/audit` 负责采集，`logs/audit` 使用独立有界队列写入
PostgreSQL `console_audit_logs`，不经过模型请求聚合。

每条记录保存服务端生成的 UUID v7 审计 ID、请求进入中间件的时间、关联 request ID、
用户 ID / 用户名 / 角色 / 租户快照、HTTP 方法、完整路径（不含 query）、响应状态码、
处理耗时、TCP 对端 IP、解析后的来源 IP 和 User-Agent。耗时从进入中间件计至产生响应，不包含响应体向
客户端传输完成的时间；HTTP 状态仅表示接口结果，不推断具体数据变更。
不采集请求体、响应体、查询字符串、Authorization 或 Cookie；request ID 最多保留
128 个字符，User-Agent 最多保留 512 个字符。关联 request ID 可以来自客户端，
唯一性以服务端生成的审计 ID 为准。

鉴权 extractor 在 JWT 校验并查到用户后写入身份快照，后续用户/租户停用或角色校验失败
仍保留该身份。登录在密码验证通过后、注册在用户创建成功后补充身份。错误密码、无效
token、未知路径及未执行身份校验的公开接口不记录推测的操作者，相关身份字段为空。
采集不增加用户查询、不读取正文，也不改变原有鉴权结果。

`peer_ip` 始终保留实际 TCP 对端地址；`client_ip` 保存按可信代理规则解析出的来源地址。
页面主要展示来源 IP，详情中保留连接对端 IP。没有 TCP 连接信息时，两者均为空。

通过 `AESTUS_TRUSTED_PROXY_IPS` 配置可信代理，支持逗号分隔的 IPv4 / IPv6 地址：

```env
AESTUS_TRUSTED_PROXY_IPS=127.0.0.1,10.0.0.10,::1
```

默认空列表，不信任任何代理。配置只接受单个 IP，不接受 CIDR、域名、端口或列表中的
空项；非法配置直接阻止启动。部署配置填写的是网关实际看到的代理 TCP 对端 IP，
容器或 NAT 场景要使用转换后的地址。可信代理必须正确覆盖或追加 `X-Forwarded-For`。

解析复用 `axum-client-addr`，只启用 XFF，不读取 `Forwarded` 或 `X-Real-IP`。
对端 IP 未命中时直接使用 `peer_ip`，忽略全部转发头；命中后按顺序处理多行 XFF，
从右向左跳过可信代理，取第一个非可信地址作为 `client_ip`。在找到该地址前遇到
无法解析的 hop、XFF 缺失或链中没有非可信地址时，记录诊断并使用 `peer_ip`，不会
拒绝请求。IPv4 与其映射 IPv6 形式在可信匹配时等价；`client_ip` 统一使用规范化地址，
`peer_ip` 保留原始 TCP 对端地址。

例如对端为可信的 `10.0.0.10`，另一个可信代理为 `10.0.0.11`，收到
`X-Forwarded-For: 198.51.100.99, 203.0.113.8, 10.0.0.11` 时，来源为 `203.0.113.8`；
更左侧的 `198.51.100.99` 不参与决定结果。解析仅使用内存中的配置和请求头，不增加
网络请求、数据库查询或异步写入等待。

平台管理员可以查看全局审计；租户 owner 只看本租户已确认身份的记录；普通用户只看
自己的记录。未确认身份的记录仅平台管理员可见。查询范围完全由登录身份决定，接口
不接受 `tenant_id` 或 `user_id` 参数。页面支持日期选择、前后翻页及请求详情；
`GET /api/console/audit-logs` 接受 `date`（服务时区自然日，默认当天）、`limit`（默认
100，最多 500），以及成对的 `before_occurred_at`、`before_id` 游标，按时间和 ID 倒序。
审计表暂不自动清理，也不使用请求日志的 30 天查询限制。

首版为尽力记录的操作追踪：请求产生响应后通过 `try_send` 投递，队列容量为 4096；
队列满或关闭时丢弃记录，写入失败仅输出诊断，不阻塞或回滚控制台操作。请求在产生响应前
被取消或 panic、进程退出时尚未写入的记录可能缺失，尚不提供与业务事务一致的审计保证。

表和索引直接定义在初始化 migration `00000000000000_init` 中，不使用外键。
已执行旧版初始化 migration 的数据库不会因再次执行 migrate 自动补表；开发环境需手动
使用空数据库重新初始化。本次没有新增历史数据迁移，也不会自动重建已有数据库。

## 目录结构

```
├── srv/       Rust 网关服务(核心)
├── plugin/    WASM 插件示例套件
├── codex-proto-test-server/ 独立的 Codex 协议转换验证服务
├── web/       React 管理面板
└── deploy/    Docker Compose 部署配置
```

## AGENTS
* 禁止添加测试用例
