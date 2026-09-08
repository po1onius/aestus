//! 可查询的业务日志：拥有聚合、去重、存储、查询与保留策略。
//!
//! 请求事实由 request 定义，协议识别由 provider 负责；本模块只消费事实，不控制请求。
//! HTTP 鉴权与查询范围由 API 层确定，运行诊断日志由 infra::logging 负责。

pub(crate) mod calendar;
pub(crate) mod policy;
pub(crate) mod request;
mod runtime;

pub(crate) use runtime::{LogsRuntime, RequestLogConsumer, start};
