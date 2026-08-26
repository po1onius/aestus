# Aestus

自托管的 AI 模型网关:统一管理上游账号资源,对外提供标准 API,内置 GPT 与 Claude 两个上游。

## 功能

- 统一 API:对外提供 OpenAI / Anthropic 风格接口
- 图片 API:提供 OpenAI 风格 `/v1/images/generations` 和 `/v1/images/edits`,复用 GPT 账号池与官方 API Key
- 账号池:自动调度与维护多个上游账号、Key
- 控制:模型白名单、用户配额、会话粘性
- 插件:上传 WASM 插件,改写请求与响应,内置了sub2api的codex协议转换
- 面板:Web 管理页管理用户、Key 与请求日志

## 技术栈

后端 Rust(axum);存储 PostgreSQL / Redis / ClickHouse;插件 WebAssembly;前端 React;部署 Docker Compose。

## 架构特点

- 模型接口与管理面板分开挂载,互不影响
- 所有上游共用一条处理流水线,差异由适配层实现
- 请求自动调度到空闲资源,失败自动换资源重试
- 插件接口编译期绑定,发布后版本不可变
- 请求全链路可追踪,各阶段事件写入 ClickHouse

## 快速开始

```bash
make dev                                        # Podman Compose 启动依赖，本地构建并启动服务
cd deploy && cp .env.example .env && docker compose up -d   # Docker 一键部署
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
