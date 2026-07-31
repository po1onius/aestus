//! 用户 token 额度异步扣减 worker。
//!
//! 请求事件路由使用 `try_send` 投递任务，队列满或关闭时允许丢失并记录 tracing。worker
//! 继续用固定并发上限消费，避免按 usage 无限制创建数据库任务。

use tokio::{
    sync::mpsc,
    task::{JoinError, JoinHandle, JoinSet},
};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::{err::AppResult, infra::db::DbPool, request_event::TokenUsage};

const QUOTA_QUEUE_CAPACITY: usize = 4096;
const QUOTA_MAX_IN_FLIGHT_TASKS: usize = 8;

/// 一次确定模型 usage 对应的强类型额度扣减任务。
#[derive(Debug)]
pub(super) struct QuotaDeductionTask {
    pub(super) request_id: Uuid,
    pub(super) user_id: Uuid,
    pub(super) api_key_id: Uuid,
    pub(super) usage: TokenUsage,
}

#[derive(Clone)]
pub(super) struct QuotaWorker {
    tx: mpsc::Sender<QuotaDeductionTask>,
}

impl QuotaWorker {
    pub(super) fn new(db_pool: DbPool) -> (Self, JoinHandle<()>) {
        let (tx, rx) = mpsc::channel(QUOTA_QUEUE_CAPACITY);
        let task = spawn_quota_worker(db_pool, rx);
        (Self { tx }, task)
    }

    /// 尝试立即投递额度任务；失败只记录诊断，绝不向请求事件路由施加背压。
    pub(super) fn dispatch(&self, task: QuotaDeductionTask) {
        let request_id = task.request_id;
        let user_id = task.user_id;
        let api_key_id = task.api_key_id;
        let total_tokens = task.usage.total_tokens;
        match self.tx.try_send(task) {
            Ok(()) => debug!(
                request_id = %request_id,
                user_id = %user_id,
                api_key_id = %api_key_id,
                total_tokens,
                "额度扣减任务已进入 worker 队列"
            ),
            Err(mpsc::error::TrySendError::Full(_)) => warn!(
                request_id = %request_id,
                user_id = %user_id,
                api_key_id = %api_key_id,
                total_tokens,
                "额度扣减 worker 队列已满，当前任务已丢弃"
            ),
            Err(mpsc::error::TrySendError::Closed(_)) => error!(
                request_id = %request_id,
                user_id = %user_id,
                api_key_id = %api_key_id,
                total_tokens,
                "额度扣减 worker 队列已关闭，当前任务已丢弃"
            ),
        }
    }
}

struct QuotaTaskCompletion {
    request_id: Uuid,
    user_id: Uuid,
    api_key_id: Uuid,
    result: AppResult<i64>,
}

fn spawn_quota_worker(
    db_pool: DbPool,
    mut rx: mpsc::Receiver<QuotaDeductionTask>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        info!(
            queue_capacity = QUOTA_QUEUE_CAPACITY,
            max_in_flight_tasks = QUOTA_MAX_IN_FLIGHT_TASKS,
            "用户额度扣减 worker 已启动"
        );

        let mut receiver_open = true;
        let mut jobs = JoinSet::<QuotaTaskCompletion>::new();
        loop {
            if !receiver_open && jobs.is_empty() {
                break;
            }

            tokio::select! {
                maybe_task = rx.recv(), if receiver_open && jobs.len() < QUOTA_MAX_IN_FLIGHT_TASKS => {
                    match maybe_task {
                        Some(task) => {
                            let task_pool = db_pool.clone();
                            jobs.spawn(async move { execute_quota_task(&task_pool, task).await });
                        }
                        None => receiver_open = false,
                    }
                }
                Some(completion) = jobs.join_next(), if !jobs.is_empty() => {
                    log_task_completion(completion);
                }
            }
        }

        warn!("用户额度扣减 worker 队列已关闭，全部在途任务已收尾");
    })
}

async fn execute_quota_task(db_pool: &DbPool, task: QuotaDeductionTask) -> QuotaTaskCompletion {
    let result = crate::user::deduct_quota(
        db_pool,
        task.request_id,
        task.user_id,
        task.api_key_id,
        task.usage,
    )
    .await;
    QuotaTaskCompletion {
        request_id: task.request_id,
        user_id: task.user_id,
        api_key_id: task.api_key_id,
        result,
    }
}

fn log_task_completion(completion: Result<QuotaTaskCompletion, JoinError>) {
    match completion {
        Ok(QuotaTaskCompletion {
            request_id,
            user_id,
            api_key_id,
            result: Ok(quota_after),
        }) => debug!(
            request_id = %request_id,
            user_id = %user_id,
            api_key_id = %api_key_id,
            quota_after,
            "用户额度扣减 worker 任务完成"
        ),
        Ok(QuotaTaskCompletion {
            request_id,
            user_id,
            api_key_id,
            result: Err(task_error),
        }) => error!(
            request_id = %request_id,
            user_id = %user_id,
            api_key_id = %api_key_id,
            error = %task_error,
            "用户额度扣减 worker 任务失败"
        ),
        Err(join_error) => error!(
            task_cancelled = join_error.is_cancelled(),
            task_panicked = join_error.is_panic(),
            error = %join_error,
            "用户额度扣减 worker 任务异常结束"
        ),
    }
}
