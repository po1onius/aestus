//! 请求日志聚合与 ClickHouse / PostgreSQL 日志写入的组合。
//!
//! 核心请求链路只发布强类型事件。本模块在 worker 任务中顺序聚合事件，最终日志快照再
//! 进入独立的 ClickHouse 批量 writer；首次策略错误在同一次收尾时派发给 PostgreSQL writer。
//! Dashboard 的 ClickHouse 读取仍由 statistics 负责。

mod lifecycle;
mod writer;

use std::sync::Arc;

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use clickhouse::Client;
use tokio::task::JoinHandle;

use crate::request::events::RequestEvent;

use super::policy_log::PolicyLogWriter;
use lifecycle::RequestLogLifecycle;
use writer::RequestLogWriter;

/// 只由请求事件消费循环持有的日志投影器。
pub(super) struct RequestLogWorker {
    lifecycle: RequestLogLifecycle,
}

impl RequestLogWorker {
    pub(super) fn new(
        client: Client,
        table: String,
        service_timezone: Tz,
        policy_log: PolicyLogWriter,
    ) -> (Self, JoinHandle<()>) {
        let table: Arc<str> = Arc::from(table);
        let (writer, writer_task) = RequestLogWriter::spawn(client, table, service_timezone);
        (
            Self {
                lifecycle: RequestLogLifecycle::new(writer, policy_log),
            },
            writer_task,
        )
    }

    pub(super) fn handle(&mut self, event: RequestEvent) {
        self.lifecycle.handle(event);
    }

    pub(super) fn evict_stale_entries(&mut self, now: DateTime<Utc>) {
        self.lifecycle.evict_stale_entries(now);
    }
}
