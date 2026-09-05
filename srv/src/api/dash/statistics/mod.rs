//! Dashboard ClickHouse 统计与分析接口。
//!
//! 本模块拥有 Dashboard 只读查询；后台 worker 只负责异步写入。请求日志明细和当前用户
//! 用量聚合按独立模块维护，后续 provider、错误率等统计继续在同级扩展。

mod calendar;
pub(super) mod gpt_account_usage;
mod request_logs;
mod usage;

use axum::Router;

use crate::state::AppState;

/// 保持现有 `/dash/request-logs` URL 的内部路由入口，避免纯代码重构影响前端。
pub(super) fn request_logs_router() -> Router<AppState> {
    request_logs::router()
}

/// 当前用户用量概览与独立时间趋势接口不依赖日志明细分页，直接使用 ClickHouse 聚合结果。
pub(super) fn usage_router() -> Router<AppState> {
    usage::router()
}
