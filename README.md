# Aestus

自托管的 AI 模型网关:统一管理上游账号资源,对外提供标准 API,内置 GPT 与 Claude 两个上游。

## 功能

- 统一 API:对外提供 OpenAI / Anthropic 风格接口
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
make dev                                        # 本地启动(需 PostgreSQL/Redis/ClickHouse)
cd deploy && cp .env.example .env && docker compose up -d   # Docker 一键部署
```

## 目录结构

```
├── srv/       Rust 网关服务(核心)
├── plugin/    WASM 插件示例套件
├── web/       React 管理面板
└── deploy/    Docker Compose 部署配置
```
