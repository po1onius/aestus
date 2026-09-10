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
- 网站首页: `/` 展示团队账号托管、分组授权、标准协议与 WASM 插件能力；提供账号资源、成员授权、请求洞察三个可切换的产品演示场景，通过控制台入口登录或注册
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

## 用户并发生命周期

用户并发按租户、用户和 Provider 跨实例汇总，同一用户的多个网关 Key 共用计数，
GPT 与 Claude 分开计算。鉴权完成后、读取请求体之前原子检查上限并登记；上传、
请求处理和内部重试都包含在同一个租约内，一次网关请求只占一个槽位。

普通响应在正文处理完成、准备返回时等待 Redis 释放，不包含已生成正文的客户端下载
时间，空正文也走同一路径。流式响应持有到响应流 EOF 或错误，等待释放后返回流终态；
客户端取消时由租约 Drop 停止续租并提交后台释放。流式插件仍在生成输出时，请求仍计入
用户并发；独立维护、日志和取消后的插件收尾不再占用槽位。处理失败同样在返回错误前
释放。租约每 30 秒续租，有效期 90 秒，进程异常退出后的遗留占用自动失效。

流水线按最终响应类型选择释放方式，SSE 不依赖 `Content-Length`、`[DONE]` 或
`response.completed` 判断请求结束。用户并发模块使用字节流组合器，无需管理 HTTP Frame。
插件绑定准备目前仍属于鉴权阶段，因此发生在并发准入之前。

## 多实例启动与上游负载

每个网关实例启动时，按 `created_at/id` 游标每批读取 256 条 PostgreSQL 资源，增量同步
Redis runtime，随后启动 maintenance 并开始接受请求。同步保留已有 ready index、投影
版本、删除标记、粘性会话和负载租约；扩容或重启不会清空其他实例正在使用的调度状态。
并发的管理操作、维护回执和启动同步由已有版本校验裁决，旧投影不能覆盖新状态或复活
已删除资源。实际同步错误会阻止当前实例启动，已经运行的实例继续服务。

上游负载按资源保存为 Redis ZSET：`provider:{provider}:resource:{kind}:{id}:leases`，
每个上游 attempt 使用独立 token，score 为 Redis 服务端时间计算的到期毫秒值。
租约有效期 90 秒，每 30 秒续租，从分配资源开始计入请求负载。普通响应完成处理后
主动释放；流式响应在网关观察到上游 EOF、错误、超时或下游取消时停止续租并独立提交
释放，不等待插件 finish、用量日志或维护回执。正常释放和取消只删除本次 token，
迟到续租不会重新创建已释放或过期的 token。
维护任务只持有资源快照，最终响应和流式输出不等待维护；重试会在释放旧 attempt 后
等待故障回执完成状态迁移和隔离，再调度下一次 attempt，等待期间不占用资源负载。
负载表达进行中的请求数，资源能否被调度由 runtime 状态决定，不能用保留已结束请求的
负载来代替故障隔离。流式释放是后台 Redis 操作，计数生效仍需任务调度和 Redis 执行。
实例退出后遗留 token 最迟在最后一次成功续租的 90 秒后不再计入负载，整个 key 的
120 秒 TTL 负责清理无人使用的资源。调度与控制台共用“清理过期租约后计数”的读取逻辑，
多个资源通过 Redis pipeline 批量读取。该负载用于资源评分，用户并发准入仍由独立租约控制。

**首次升级到此实现需要统一切换全部旧网关实例。** 先在入口停止向旧实例分发新请求，
等待在途请求结束，再停止全部旧实例并启动新版本。旧版本启动仍会清空共享调度状态，
且使用不同的负载结构，因此不能与新版本混跑；完成这次切换后可正常扩容或滚动重启。
本次无需 PostgreSQL migration，也无需清空 Redis。旧版 `provider:{provider}:resource:load`
和 `provider:{provider}:resource:lease` Hash 不再被新版本访问，可在确认全部旧实例退出后
手动删除；保留它们不影响新版本。请保留 runtime、ready index、版本和删除标记。

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

平台管理员可在创建租户时配置限制，也可通过租户列表的“设置限制”修改：

- **租户用户数上限**：包括 owner、普通用户及停用用户。租户码注册、owner 添加用户、
  平台创建租户时初始化 owner 都受限制；设置为 0 时不能同时创建 owner。
- **每用户网关 Key 上限**：由租户统一配置，owner 和每位普通用户分别计数，跨 Provider、
  分组合计。停用 Key、分组或套件绑定失效的 Key 仍占名额，删除 Key 后释放；上游官方
  API Key 不计入此限制。
- **允许 owner 上传 WASM**：只控制新增插件上传。关闭后已有插件、套件仍可使用和管理，
  owner 仍可使用已有公共及私有插件创建套件。平台管理员上传公共插件不受影响。

两项数量默认不限制，支持 `null`（不限制）或 `0..=2147483647` 整数，0 表示禁止新增；
新租户默认禁止 owner 上传 WASM。允许把上限调到当前数量以下，现有用户和 Key 保留，
只有新增时检查上限。用户创建、Key 创建和插件落库在事务中先锁定租户，使用最新限制；
平台更新限制持有同一个租户行锁，保证多实例并发操作不会突破上限。WASM 上传在读取文件
和编译前预检查权限，编译后落库时再次检查，编译期间不持有租户行锁。

`POST /api/console/tenants` 接受可选 `limits` 对象；省略整个对象时使用上述默认值。
`PUT /api/console/tenants/{id}/limits` 仅平台管理员可调用，请求体直接为整组限制：

```json
{
  "max_users": 100,
  "max_gateway_keys_per_user": 5,
  "owner_can_upload_wasm": false
}
```

租户列表返回这三项字段及 `user_count`，用户数按租户批量聚合。登录、注册和 `/auth/me`
响应的 `tenant` 也包含限制字段。用户、网关 Key 和插件页面在进入及刷新时读取最新配置；
实际写入始终以后端事务校验为准。用户数或 Key 数超限返回 HTTP 409，错误码分别为
`tenant_user_limit_exceeded`、`tenant_gateway_key_limit_exceeded`；禁止 WASM 上传返回
HTTP 403 和 `tenant_wasm_upload_forbidden`。限制修改记录操作者及修改前后值，拒绝新增
记录租户、数量及上限，日志不记录 Key 明文或 WASM 内容。

三项字段直接定义在初始化 migration `00000000000000_init` 的 `tenants` 表中，不使用
外键。本次没有新增历史数据 migration，也不会自动修改现有数据库；已执行旧版初始化的
开发数据库需手动使用空数据库重新初始化。需要保留历史生产数据时，必须先确认迁移方案
再升级，不能只重复运行已有 migration。前后端需同步发布。

控制台分组列表按本次可见分组 ID 批量统计账号、上游 API Key 和网关 Key；网关 Key 的
总数与启用数量在同一条 SQL 中聚合。非空列表的数据查询固定为 5 次（不含鉴权），空列表
只查询分组表；无关联资源的计数为零。统计口径、分组排序和返回结构保持一致。

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
仅预览前端时，执行 `cd web && npm run dev`，访问 `http://127.0.0.1:5173/`。
首页产品预览使用明确标注的固定演示数据，不请求业务接口；控制台登录与操作仍需启动后端。

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

前端 App 只组装会话、权限路由、主题和控制台布局。各业务页面按需加载，由页面内的
Screen 管理数据、分页、业务操作和弹窗，表单输入由弹窗自身持有。登录后只加载当前页面
所需数据；资源列表只请求当前 Provider 和凭证类型，翻页不再刷新其他三类资源。
修改当前用户的额度或并发上限不会触发其他页面重载。

页面切换会销毁原页面的分页、筛选和未提交表单，重新进入时读取最新数据；退出或更换
登录会话会销毁整个控制台状态。迁移后的页面请求随页面取消，同类查询仅接受最新结果，
过期响应不能覆盖新数据或提前结束新请求的加载状态。取消客户端等待不会回滚服务端已
提交的写操作，不会自动重试写操作。分组选项与管理统计继续使用各自权限对应的接口。

控制台和模型网关使用独立的错误响应边界。`err::ConsoleError` 定义控制台鉴权、权限及
租户限制等业务错误，由 `AppError::Console` 包装；`AppError` 只保留错误事实与诊断 code，
不依赖 HTTP，也不实现 `IntoResponse`。控制台 handler 和鉴权 extractor 使用
`api/console/error` 的 `ConsoleResult` / `ConsoleApiError`，负责状态码、公开信息、脱敏
详情和错误日志。模型网关由 `api/gateway/error` 完成公开错误映射，Provider 只接收
`ProviderVisibleError` 并编码为各自协议。新增控制台业务错误只需扩展控制台错误枚举和
控制台响应映射；网关统一处理整个 `Console(_)` 分组，不逐项匹配管理业务错误。

网关日志使用实际对外响应状态码；内部诊断和公开错误分别记录。已注册端点无法识别
Provider 属于路由声明不一致，返回脱敏的 500 并记录诊断；普通未知路径和方法仍由 Router
返回 404/405。此次错误边界重构无需数据库迁移。

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
资源都能一致执行的 buffered `gpt-image-2.5` 子集：`model` 可以省略，省略时按
`gpt-image-2.5` 授权并向上游显式补齐；显式模型只接受 `gpt-image-2.5`。共同支持
`prompt`、`background`、`n`、`quality` 和 `size`，不支持 `stream=true`，其他参数会返回
请求错误而不会被静默忽略。

generations 接收 JSON；edits 接收 `multipart/form-data`，支持单个或多个 `image` /
`image[]` 文件字段，最多 16 张；每张必须是小于 50 MiB 的 PNG、JPEG 或 WebP。当前跨
Account 与 Official API Key 一致的编辑子集不包含 `mask`、`input_fidelity`、
`output_format` 等额外参数。

账号资源会请求 Codex `/images/generations`；官方 API Key 资源请求其 Base URL 下的相同
路径。图片编辑分别请求 `/images/edits`；调度前完成一次 multipart 解析、Base64 编码和
JSON 转换，转换结果在同一次网关请求的全部尝试中复用。两类资源分别在这份不可变正文上
应用自己的 resource override，并在覆盖后校验最终 Images JSON，不会沿用上一次尝试的
覆盖结果。两个图片上游路径可分别通过
`AESTUS_GPT_UPSTREAM_IMAGE_GENERATIONS_PATH` 和 `AESTUS_GPT_UPSTREAM_IMAGE_EDITS_PATH`
覆盖。

```bash
curl http://127.0.0.1:8080/v1/images/generations \
  -H 'authorization: Bearer <AESTUS_GATEWAY_KEY>' \
  -H 'content-type: application/json' \
  -d '{"model":"gpt-image-2.5","prompt":"一只坐在月球上的橘猫","size":"1024x1024"}'
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

原生 GPT SSE 按到达顺序增量扫描 LF / CRLF 事件边界，每个事件的 JSON 文本只解析一次，
用量、错误分类和策略日志复用解析结果。完整事件直接从字节缓冲切分，单行 `data:` 借用
原始内容；透传保留上游原始字节，需转换的资源故障仍输出既有 client retry 事件。

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

GPT 账号额度查询分为两个接口，均使用 POST：

- `/api/console/providers/gpt/accounts/{id}/quota` 沿用分组额度查看权限，只返回上游额度，
  不查询请求日志，响应不包含网关 Token 总量或用户用量字段。
- `/api/console/providers/gpt/accounts/{id}/quota-with-usage` 仅允许租户 owner 查询本租户
  账号；在上游额度响应之外增加 `gateway_usage`，其 `primary` / `secondary` 分别对应主
  Codex 额度的两个窗口。每个可统计窗口包含字符串 `total_tokens` 和 `users` 列表；用户项
  包含 `user_id`、`username`、字符串 `total_tokens`。普通用户和平台管理员访问此接口返回
  403，客户端不能指定租户。两个接口共用上游查询及额度恢复后的调度状态同步逻辑。

控制台根据角色选择接口，只有 owner 的额度弹窗展示“本窗口网关已记录 Token”和各用户
用量。窗口起点由上游重置时间减去原始窗口秒数得到，按账号汇总从起点（包含）到额度查询
时间（不包含）之间开始的请求，不按请求状态、模型或功能过滤。同一请求可计入两个窗口。
一次日志查询按用户 ID 分别聚合两个窗口，窗口总量由同一批分组结果求和；用户名采用各窗口
内该用户最新请求的日志快照，改名不会拆分统计，用户删除后已记录的用量仍保留。
用户列表仅展示用量大于零的分组，按 Token 降序排列，并展示用户 Token 占本网关该窗口
Token 总量的百分比（保留两位小数，小于 0.01% 的非零占比显示 `<0.01%`）。
日志缺少用户 ID 的用量保留为未记录
用户分组。该数字只包含本网关已落库的用量；窗口时间无效、已过期或起点超出日志保留期时，
对应 `gateway_usage` 窗口为 `null`，前端显示“无法统计”；可查询但无用量时总量为 `"0"`、
用户列表为空。额外额度项暂不推断模型或功能归属，人工重置后仍按上游返回的窗口时间计算。
本次变更无需数据库迁移；前后端需要同步发布，以使用新的 owner 接口和响应结构。

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

通过 `AESTUS_TRUSTED_PROXY_IPS` 配置可信代理，支持逗号分隔的 IPv4 / IPv6 单个地址与 CIDR 网段混填：

```env
AESTUS_TRUSTED_PROXY_IPS=127.0.0.1,172.18.0.0/16,::1,fd00:1234::/64
```

默认空列表，不信任任何代理。单个 IP 等价于 IPv4 的 `/32` 或 IPv6 的 `/128`，
CIDR 按配置前缀匹配整个网段。CIDR 必须填写网络地址，例如 `172.18.0.0/16`；
`172.18.0.1/16` 这样的主机位非零地址、非法前缀、域名、端口和列表中的空项均会阻止启动。
部署配置应覆盖网关实际看到的代理 TCP 对端 IP，容器或 NAT 场景要使用转换后的地址。
配置网段表示信任其中所有地址提供的 XFF；可信代理必须正确覆盖或追加 `X-Forwarded-For`。

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
