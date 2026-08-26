# Codex 协议测试服务

本目录是独立的 Cargo 项目，不属于 `plugin` workspace。服务通过 path dependency 直接复用
`plugin/crates/common` 的转换实现，用于真实调用 Codex 账号端点并验证 Responses 插件转换。

服务提供本地 `POST /v1/responses`。它会在返回前检查最终 JSON/SSE 是否符合 Responses API
的核心结构；校验失败返回 502 和 `plugin_test_error`。日志会记录请求大小、模型、上游状态
和具体校验错误，但不会记录 access token、refresh token 或完整 prompt。

服务固定读取本目录中的 `token.toml`，不再通过启动参数接收 OAuth 凭证。文件只包含三个字段；
其中 `accese_token` 按当前配置协议保留该拼写：

```toml
accese_token = ""
refresh_token = "<CODEX_REFRESH_TOKEN>"
client_id = "app_EMoamEEZ73f0CkXaXp7hrann"
```

`accese_token` 非空时直接使用，不会消费 refresh token。`accese_token` 为空时，服务会在开始
监听前使用 refresh token 请求 `https://auth.openai.com/oauth/token`；`client_id` 为空时使用
Codex CLI 当前默认值 `app_EMoamEEZ73f0CkXaXp7hrann`。

刷新成功后，服务会把新 access token 回填到 `accese_token`；OAuth 服务如果同时返回轮换后的
refresh token，也会一起更新，避免下次启动继续使用已经失效的旧凭证。`token.toml` 已被 Git
忽略，日志不会记录其中任何 token。

```bash
cargo run --manifest-path codex-proto-test-server/Cargo.toml
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

Responses 上游地址可通过 `--upstream-url` 覆盖。
