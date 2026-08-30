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
- 面板:Web 管理页管理用户、网关Api Key、账号、分组、额度、请求日志

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
网关只替换所选 Account 或 Official API Key 的凭证；当前不解释上游错误响应，也不记录
token usage。Codex model provider 的 Base URL 指向 Aestus 的 `/v1` 后，会自动请求该接口。

GPT 搜索上游路径默认是 `/alpha/search`，可通过 `AESTUS_GPT_UPSTREAM_SEARCH_PATH` 覆盖。

## 请求日志与用量

ClickHouse 请求明细默认保留 30 天，可通过 `AESTUS_REQUEST_LOG_RETENTION_DAYS` 配置；服务启动时
会自动同步表 TTL，调小后超期明细将由 ClickHouse 后台异步删除。管理员全局时间线使用主排序键，普通用户查询使用
`user_id` 轻量投影。请求写入时会按 `AESTUS_TIMEZONE` 计算固定的业务日，并通过增量物化
视图写入长期保留的日用量聚合表。Dashboard 的全历史总量、模型、API Key、用户分布和最近
365 天均只查询该聚合表。

`AESTUS_TIMEZONE` 必须是 IANA 时区，例如 `UTC` 或 `Asia/Shanghai`。它定义全部用户共用的业务日
边界；产生聚合数据后修改该值需要重建日聚合，不应将它当作普通的运行时开关。

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
