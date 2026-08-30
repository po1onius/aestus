//! 三个转换插件允许共用的纯工具函数。
//!
//! 本 crate 不定义任何请求、响应、usage、maintenance 或重试业务语义。各插件必须在
//! 自己的 crate 内完成协议判断，只能复用这里的通用 SSE framing 解析与渲染能力。

pub mod sse;
