//! 非核心后台功能的组合根。
//!
//! 核心请求链路只持有 [`RequestEventPublisher`] 并非阻塞地发布事实。本模块私有持有事件
//! receiver、日志消费者和额度执行器。日志聚合及写入生命周期由 logs 模块拥有。

mod quota;

use chrono_tz::Tz;
use clickhouse::Client as ClickHouseClient;
use tokio::{sync::mpsc, task::JoinHandle};
use tracing::{info, warn};

use crate::{
    infra::db::DbPool,
    logs::{self, LogsRuntime, RequestLogConsumer},
    request::events::{RequestEvent, RequestEventPublisher},
};

use quota::{QuotaDeductionTask, QuotaWorker};

const REQUEST_EVENT_QUEUE_CAPACITY: usize = 4096;

/// 服务进程持有的全部非核心 worker 任务。
///
/// `AppState` 不持有本类型，避免业务模块取得 worker 实现。组合根在 HTTP 服务的完整
/// 生命周期内保留它；退出或后续启动失败时 Drop 会停止所有后台任务。
pub struct WorkerRuntime {
    tasks: Vec<JoinHandle<()>>,
    _logs: LogsRuntime,
}

impl Drop for WorkerRuntime {
    fn drop(&mut self) {
        let task_count = self.tasks.len();
        for task in &self.tasks {
            task.abort();
        }
        info!(task_count, "非核心 worker 后台任务已停止");
    }
}

/// 启动请求事件路由及其独立后台消费者。
pub fn start(
    db_pool: DbPool,
    clickhouse: ClickHouseClient,
    request_log_table: String,
    service_timezone: Tz,
) -> (RequestEventPublisher, WorkerRuntime) {
    let (publisher, event_rx) = RequestEventPublisher::channel(REQUEST_EVENT_QUEUE_CAPACITY);
    let (request_log, logs_runtime) = logs::start(
        db_pool.clone(),
        clickhouse,
        request_log_table,
        service_timezone,
    );
    let (quota, quota_task) = QuotaWorker::new(db_pool);
    let event_router_task = spawn_request_event_router(event_rx, request_log, quota);

    info!(
        queue_capacity = REQUEST_EVENT_QUEUE_CAPACITY,
        service_timezone = %service_timezone,
        "非核心 worker 请求事件入口已启动"
    );
    (
        publisher,
        WorkerRuntime {
            tasks: vec![event_router_task, quota_task],
            _logs: logs_runtime,
        },
    )
}

fn spawn_request_event_router(
    mut rx: mpsc::Receiver<RequestEvent>,
    mut request_log: RequestLogConsumer,
    quota: QuotaWorker,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                maybe_event = rx.recv() => {
                    let Some(event) = maybe_event else {
                        break;
                    };

                    // usage 对两个后台消费者都是独立事实。额度任务携带完整归属，不读取
                    // 请求日志投影；额度队列满载也不会阻止日志继续消费同一事件。
                    if let RequestEvent::UsageObserved {
                        request_id,
                        attribution,
                        usage,
                    } = &event
                    {
                        quota.dispatch(QuotaDeductionTask {
                            request_id: *request_id,
                            user_id: attribution.user_id,
                            api_key_id: attribution.api_key_id,
                            usage: *usage,
                        });
                    }
                    request_log.handle(event);
                }
                () = request_log.maintain() => {}
            }
        }

        warn!("后台请求事件队列已关闭，事件路由任务结束");
    })
}
