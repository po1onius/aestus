# Codex 协议测试服务

本目录是独立的 Cargo 项目，不属于 `plugin` workspace。服务通过 path dependency 直接复用
`plugin/crates/common` 的转换实现，用于真实调用 Codex 账号端点并验证 Responses 插件转换。

服务提供本地 `POST /v1/responses`。它会在返回前检查最终 JSON/SSE 是否符合 Responses API
的核心结构；校验失败返回 502 和 `plugin_test_error`。日志会记录请求大小、模型、上游状态
和具体校验错误，但不会记录 access token、refresh token 或完整 prompt。

启动时必须提供 access token 或 refresh token。显式 access token 的优先级更高；同时提供
两者时不会消费 refresh token：

```bash
cargo run --manifest-path codex-proto-test-server/Cargo.toml -- \
  --access-token "$CODEX_ACCESS_TOKEN"
```

未提供 access token 时，服务会在开始监听前使用 refresh token 请求
`https://auth.openai.com/oauth/token`：

```bash
cargo run --manifest-path codex-proto-test-server/Cargo.toml -- \
  --refresh-token "$CODEX_REFRESH_TOKEN"
```

`--client-id` 默认是 Codex CLI 当前使用的 `app_EMoamEEZ73f0CkXaXp7hrann`，需要覆盖时可传：

```bash
cargo run --manifest-path codex-proto-test-server/Cargo.toml -- \
  --refresh-token "$CODEX_REFRESH_TOKEN" \
  --client-id "$CODEX_CLIENT_ID"
```

OAuth 服务可能同时返回轮换后的 refresh token。测试服务不会记录或持久化任何 token，调用方
如需长期管理和轮换登录凭证，应继续使用 Codex CLI 的凭证存储，不要把测试服务作为凭证仓库。

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
