//! Dashboard 日志查询、统计与分析接口。
//!
//! 日志接口负责 HTTP 参数和授权范围，具体读写归 logs 模块；用量统计独立查询日聚合。

pub(super) mod gpt_account_usage;
mod policy_logs;
mod request_logs;
mod usage;

use axum::Router;

use crate::state::AppState;

/// 普通请求日志明细查询。
pub(super) fn request_logs_router() -> Router<AppState> {
    request_logs::router()
}

/// Policy 日志独立查询，保留自身的数据范围、权限和分页规则。
pub(super) fn policy_logs_router() -> Router<AppState> {
    policy_logs::router()
}

/// 当前用户用量概览与独立时间趋势接口不依赖日志明细分页，直接使用 ClickHouse 聚合结果。
pub(super) fn usage_router() -> Router<AppState> {
    usage::router()
}
