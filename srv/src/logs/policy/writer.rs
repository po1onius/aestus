//! 独立的 GPT 策略日志 PostgreSQL writer；队列及落库失败不会反向影响模型请求。

use chrono::{DateTime, Utc};
use diesel::prelude::*;
use diesel_async::RunQueryDsl;
use tokio::{sync::mpsc, task::JoinHandle};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::{
    err::{AppError, AppResult},
    infra::db::{DbPool, get_connection},
    provider::gpt::sql::account::find_email_by_id,
    request::events::GptPolicyErrorCode,
};

use super::model::gpt_policy_violation_logs;

const POLICY_LOG_QUEUE_CAPACITY: usize = 4096;

pub(in crate::logs) struct PolicyLogTask {
    pub request_id: Uuid,
    pub tenant_id: String,
    pub username: String,
    /// 仅用于 writer 查询账号邮箱；policy 表不保存资源 ID，ClickHouse 仍保存它。
    pub resource_id: Option<Uuid>,
    pub occurred_at: DateTime<Utc>,
    pub error_code: GptPolicyErrorCode,
}

pub(in crate::logs) struct PolicyLogWriter {
    tx: mpsc::Sender<PolicyLogTask>,
}

impl PolicyLogWriter {
    pub(in crate::logs) fn new(db_pool: DbPool) -> (Self, JoinHandle<()>) {
        let (tx, mut rx) = mpsc::channel::<PolicyLogTask>(POLICY_LOG_QUEUE_CAPACITY);
        let task = tokio::spawn(async move {
            info!(
                queue_capacity = POLICY_LOG_QUEUE_CAPACITY,
                "GPT 策略日志 worker 已启动"
            );
            while let Some(task) = rx.recv().await {
                match insert_policy_log(&db_pool, &task).await {
                    Ok(()) => debug!(
                        request_id = %task.request_id,
                        tenant_id = %task.tenant_id,
                        username = %task.username,
                        resource_id = ?task.resource_id,
                        occurred_at = %task.occurred_at,
                        error_code = task.error_code.as_str(),
                        "GPT 策略日志已写入 PostgreSQL"
                    ),
                    Err(error) => error!(
                        request_id = %task.request_id,
                        tenant_id = %task.tenant_id,
                        username = %task.username,
                        resource_id = ?task.resource_id,
                        occurred_at = %task.occurred_at,
                        error_code = task.error_code.as_str(),
                        %error,
                        "GPT 策略日志写入 PostgreSQL 失败"
                    ),
                }
            }
            warn!("GPT 策略日志 worker 队列已关闭，任务结束");
        });
        (Self { tx }, task)
    }

    pub(in crate::logs) fn dispatch(&self, task: PolicyLogTask) {
        let request_id = task.request_id;
        let resource_id = task.resource_id;
        let error_code = task.error_code.as_str();
        match self.tx.try_send(task) {
            Ok(()) => {
                debug!(%request_id, ?resource_id, error_code, "GPT 策略日志已进入 writer 队列")
            }
            Err(mpsc::error::TrySendError::Full(_)) => warn!(
                %request_id, ?resource_id, error_code, "GPT 策略日志 writer 队列已满，当前日志已丢弃"
            ),
            Err(mpsc::error::TrySendError::Closed(_)) => error!(
                %request_id, ?resource_id, error_code, "GPT 策略日志 writer 队列已关闭，当前日志已丢弃"
            ),
        }
    }
}

async fn insert_policy_log(db_pool: &DbPool, task: &PolicyLogTask) -> AppResult<()> {
    use gpt_policy_violation_logs::dsl;

    let mut conn = get_connection(db_pool).await?;
    let account_email = match task.resource_id {
        Some(resource_id) => find_email_by_id(&mut conn, &task.tenant_id, resource_id).await?,
        None => None,
    };
    if account_email.is_none() {
        warn!(
            request_id = %task.request_id,
            tenant_id = %task.tenant_id,
            resource_id = ?task.resource_id,
            "策略日志缺少资源 ID、账号已删除或账号未记录邮箱，邮箱字段保存 NULL"
        );
    }
    diesel::insert_into(dsl::gpt_policy_violation_logs)
        .values((
            dsl::tenant_id.eq(&task.tenant_id),
            dsl::username.eq(&task.username),
            dsl::account_email.eq(account_email),
            dsl::occurred_at.eq(task.occurred_at),
            dsl::error_code.eq(task.error_code.as_str()),
        ))
        .execute(&mut conn)
        .await
        .map_err(|source| AppError::DbQuery {
            message: format!("写入 GPT 策略日志失败: {source}"),
        })?;
    Ok(())
}
