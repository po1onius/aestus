//! 日志消费者及其私有 writer 任务的生命周期。

use std::{sync::Arc, time::Duration};

use chrono::Utc;
use chrono_tz::Tz;
use clickhouse::Client;
use tokio::{
    task::JoinHandle,
    time::{self, Interval},
};
use tracing::info;

use super::{
    policy::PolicyLogWriter,
    request::{lifecycle::RequestLogLifecycle, writer::RequestLogWriter},
};
use crate::{infra::db::DbPool, request::events::RequestEvent};

const REQUEST_LOG_STALE_SWEEP_INTERVAL_SECONDS: u64 = 60;

/// 由事件路由顺序调用；聚合状态与回收周期全部由日志模块维护。
pub(crate) struct RequestLogConsumer {
    lifecycle: RequestLogLifecycle,
    stale_sweep: Interval,
}

impl RequestLogConsumer {
    pub(crate) fn handle(&mut self, event: RequestEvent) {
        self.lifecycle.handle(event);
    }

    /// 供事件路由的 select 使用；等待 tick 可取消，不创建额外事件队列。
    pub(crate) async fn maintain(&mut self) {
        self.stale_sweep.tick().await;
        self.lifecycle.evict_stale_entries(Utc::now());
    }
}

/// 进程组合根持有此守卫，退出或后续启动失败时停止所有日志 writer。
pub(crate) struct LogsRuntime {
    tasks: Vec<JoinHandle<()>>,
}

impl Drop for LogsRuntime {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
        info!(task_count = self.tasks.len(), "业务日志后台写入任务已停止");
    }
}

pub(crate) fn start(
    db_pool: DbPool,
    clickhouse: Client,
    request_log_table: String,
    service_timezone: Tz,
) -> (RequestLogConsumer, LogsRuntime) {
    let (policy_log, policy_log_task) = PolicyLogWriter::new(db_pool);
    let (writer, writer_task) =
        RequestLogWriter::spawn(clickhouse, Arc::from(request_log_table), service_timezone);
    let mut stale_sweep = time::interval(Duration::from_secs(
        REQUEST_LOG_STALE_SWEEP_INTERVAL_SECONDS,
    ));
    stale_sweep.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
    info!(%service_timezone, stale_sweep_interval_seconds = REQUEST_LOG_STALE_SWEEP_INTERVAL_SECONDS, "业务日志消费者已启动");
    (
        RequestLogConsumer {
            lifecycle: RequestLogLifecycle::new(writer, policy_log),
            stale_sweep,
        },
        LogsRuntime {
            tasks: vec![writer_task, policy_log_task],
        },
    )
}
