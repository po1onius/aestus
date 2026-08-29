# Codex 协议测试服务

本目录是独立的 Cargo 项目，不属于 `plugin` workspace。服务通过 path dependency 分别调用
`plugin/crates/request`、`buffered-response` 和 `stream-response` 的独立转换入口，并只从
`plugin/crates/utils` 复用通用 SSE 工具，用于真实调用 Codex 账号端点并验证 Responses 插件转换。

服务提供本地 `POST /v1/responses`。它会在返回前检查最终 JSON/SSE 是否符合 Responses API
的核心结构；校验失败返回 502 和 `plugin_test_error`。日志会记录请求大小、模型、上游状态
和具体校验错误，但不会记录 access token、refresh token 或完整 prompt。

服务固定读取本目录中的 `token.toml`，不再通过启动参数接收 OAuth 凭证：

```toml
access_token = ""
refresh_token = "<CODEX_REFRESH_TOKEN>"
client_id = "app_EMoamEEZ73f0CkXaXp7hrann"
chatgpt_account_id = ""
```

`access_token` 非空时直接使用，不会消费 refresh token。`access_token` 为空时，服务会在开始
监听前使用 refresh token 请求 `https://auth.openai.com/oauth/token`；`client_id` 为空时使用
Codex CLI 当前默认值 `app_EMoamEEZ73f0CkXaXp7hrann`。

刷新成功后，服务会把新 access token 回填；OAuth 服务如果同时返回轮换后的 refresh token，
也会一起更新，避免下次启动继续使用已经失效的旧凭证。服务还会依次从刷新响应的显式字段、
`id_token` 和 `access_token` 提取 `chatgpt_account_id`：提取成功就覆盖并回填配置；没有提取到
则保留配置中的原值；两处都为空时不发送 `ChatGPT-Account-ID` header。

`token.toml` 已被 Git 忽略，日志不会记录其中任何 token 或 Account ID。

```bash
cargo run --manifest-path codex-proto-test-server/Cargo.toml
```

多 workspace 调试时可以额外传入 `--chatgpt-account-id <ACCOUNT_ID>`，它只覆盖本次进程实际
使用的值，不会回写配置。监听地址默认为 `127.0.0.1:3000`。非流式验证示例：

```bash
curl http://127.0.0.1:3000/v1/responses \
  -H 'content-type: application/json' \
  -d '{"model":"gpt-5.6-terra","input":"只回复 OK","stream":false}'
```

流式验证只需把请求中的 `stream` 改为 `true`。测试服务为了能在写出 HTTP 响应前完成整条
事件序列和终止事件校验，会先收集完整上游 SSE，再以 `text/event-stream` 返回；它用于验证
插件协议转换，不用于测量首 token 延迟。

## 请求调试记录

每个进入 `/v1/responses` 的请求都会在本目录的 `trace/` 下创建一个独立调试文件。文件名使用
精确到秒的 UTC 时间，例如 `2026-08-28_12-34-56_UTC.trace.log`；同一秒内存在多个请求时会
自动追加递增序号。文件以 append 模式打开，并按处理顺序记录：

- 美化后的原始下游 JSON 请求体；
- 请求插件转换后实际发送给 Codex 的 JSON 请求体；
- Codex 上游响应状态，以及每个完整 SSE event 的 `data:` 反序列化 JSON；
- 流式 SSE 转非流式响应时，最终生成的完整 Responses JSON；
- 请求转换、上游请求、SSE 切分或反序列化、响应转换和最终协议校验期间发生的错误。

非法 JSON/SSE 会同时记录解析错误和原始内容，便于直接定位协议问题。调试文件不会写入 HTTP
header，因此不会包含 OAuth token、下游鉴权信息或 ChatGPT account header；请求和响应正文
可能包含完整 prompt 及模型输出，只应在受控的本地调试环境中保存。

Responses 上游地址可通过 `--upstream-url` 覆盖。

## AGENTS
测试固定使用gpt-5.6-terra模型
