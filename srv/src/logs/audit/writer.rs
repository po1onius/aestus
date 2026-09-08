//! 审计独立使用有界队列，记录失败只输出诊断，不改变控制台操作结果。

use diesel_async::RunQueryDsl;
use tokio::{sync::mpsc, task::JoinHandle};
use tracing::{debug, error, info, warn};

use super::model::{AuditLogRecord, console_audit_logs};
use crate::infra::db::{DbPool, get_connection};

const AUDIT_LOG_QUEUE_CAPACITY: usize = 4096;

#[derive(Clone)]
pub(crate) struct AuditLogPublisher {
    tx: mpsc::Sender<AuditLogRecord>,
}

impl AuditLogPublisher {
    pub(in crate::logs) fn start(db_pool: DbPool) -> (Self, JoinHandle<()>) {
        let (tx, mut rx) = mpsc::channel::<AuditLogRecord>(AUDIT_LOG_QUEUE_CAPACITY);
        let task = tokio::spawn(async move {
            info!(
                queue_capacity = AUDIT_LOG_QUEUE_CAPACITY,
                "控制台审计 writer 已启动"
            );
            while let Some(record) = rx.recv().await {
                let result = async {
                    let mut conn = get_connection(&db_pool).await?;
                    diesel::insert_into(console_audit_logs::table)
                        .values(&record)
                        .execute(&mut conn)
                        .await
                        .map_err(|source| crate::err::AppError::DbQuery {
                            message: format!("写入控制台审计失败: {source}"),
                        })?;
                    Ok::<(), crate::err::AppError>(())
                }
                .await;
                match result {
                    Ok(()) => {
                        debug!(audit_id = %record.id, user_id = ?record.user_id, status_code = record.status_code, "控制台审计已写入 PostgreSQL")
                    }
                    Err(error) => {
                        error!(audit_id = %record.id, user_id = ?record.user_id, %error, "控制台审计写入失败，当前记录已丢弃")
                    }
                }
            }
            warn!("控制台审计队列已关闭，writer 结束");
        });
        (Self { tx }, task)
    }

    pub(crate) fn emit(&self, record: AuditLogRecord) {
        let audit_id = record.id;
        match self.tx.try_send(record) {
            Ok(()) => debug!(%audit_id, "控制台审计已进入写入队列"),
            Err(mpsc::error::TrySendError::Full(_)) => {
                warn!(%audit_id, "控制台审计队列已满，当前记录已丢弃")
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                error!(%audit_id, "控制台审计队列已关闭，当前记录已丢弃")
            }
        }
    }
}
