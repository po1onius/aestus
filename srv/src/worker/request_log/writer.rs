use std::{sync::Arc, time::Duration};

use chrono::{DateTime, NaiveDate, Utc};
use chrono_tz::Tz;
use clickhouse::{Client, Row};
use serde::{Deserialize, Serialize};
use tokio::{sync::mpsc, task::JoinHandle, time};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::err::{AppError, AppResult};

use super::lifecycle::FinalizedRequestLogEntry;

const REQUEST_LOG_QUEUE_CAPACITY: usize = 4096;
// 常规批次累计到 1,000 行后立即提交，减少小批量同步 INSERT 产生的 data part 数量和
// ClickHouse 后台 merge 压力。字节上限仍独立生效，避免少量超大错误正文占用过多内存。
const REQUEST_LOG_BATCH_MAX_ROWS: usize = 1_000;
const REQUEST_LOG_BATCH_MAX_ESTIMATED_BYTES: usize = 2 * 1024 * 1024;
// 低流量下最多等待 2 秒便提交已有日志，在合批效率和 Dashboard 可见延迟之间保持平衡。
const REQUEST_LOG_BATCH_FLUSH_INTERVAL_MS: u64 = 2_000;

/// ClickHouse 请求日志存储行。
///
/// `extra` 仍以 JSON 字符串保存低频扩展诊断；稳定筛选字段全部是独立列。该类型只负责
/// writer 编码，Dashboard 查询使用独立只读 DTO，避免读写职责重新耦合。
#[derive(Debug, Deserialize, Serialize, Row)]
pub(super) struct RequestLogRow {
    #[serde(with = "clickhouse::serde::uuid")]
    pub(super) request_id: Uuid,
    pub(super) provider: String,
    pub(super) route: String,
    pub(super) api_key_name: Option<String>,
    pub(super) tenant_id: Option<String>,
    #[serde(with = "clickhouse::serde::uuid::option")]
    pub(super) user_id: Option<Uuid>,
    pub(super) username: Option<String>,
    #[serde(with = "clickhouse::serde::uuid::option")]
    pub(super) provider_group_id: Option<Uuid>,
    pub(super) provider_group_name: Option<String>,
    pub(super) model: Option<String>,
    pub(super) reasoning: Option<String>,
    pub(super) service_tier: Option<String>,
    pub(super) fast_mode: Option<bool>,
    pub(super) is_compaction: Option<bool>,
    #[serde(with = "clickhouse::serde::chrono::date")]
    pub(super) usage_date: NaiveDate,
    #[serde(with = "clickhouse::serde::chrono::datetime64::millis")]
    pub(super) request_started_at: DateTime<Utc>,
    #[serde(with = "clickhouse::serde::chrono::datetime64::millis::option")]
    pub(super) response_started_at: Option<DateTime<Utc>>,
    #[serde(with = "clickhouse::serde::chrono::datetime64::millis::option")]
    pub(super) response_finished_at: Option<DateTime<Utc>>,
    pub(super) input_tokens: i64,
    pub(super) cached_input_tokens: i64,
    pub(super) output_tokens: i64,
    pub(super) reasoning_output_tokens: i64,
    pub(super) total_tokens: i64,
    pub(super) status: String,
    pub(super) extra: String,
}

impl RequestLogRow {
    /// 估算后台批次内存，只用于设置保守上限，不作为精确内存计量。
    fn estimated_bytes(&self) -> usize {
        const FIXED_FIELD_BYTES: usize = 256;

        FIXED_FIELD_BYTES
            + self.provider.len()
            + self.route.len()
            + self.api_key_name.as_ref().map_or(0, String::len)
            + self.username.as_ref().map_or(0, String::len)
            + self.provider_group_name.as_ref().map_or(0, String::len)
            + self.model.as_ref().map_or(0, String::len)
            + self.reasoning.as_ref().map_or(0, String::len)
            + self.service_tier.as_ref().map_or(0, String::len)
            + self.status.len()
            + self.extra.len()
    }
}

impl RequestLogRow {
    fn try_from_finalized(
        finalized: FinalizedRequestLogEntry,
        service_timezone: Tz,
    ) -> Result<Self, serde_json::Error> {
        let FinalizedRequestLogEntry { entry, status } = finalized;
        let usage = entry.token_usage.unwrap_or_default();
        // success/abnormal/failed 已由 worker lifecycle 在收到原子终态时确定；writer
        // 只负责稳定序列化，不再从 extra 的可选字段反推业务结果。
        let status = status.as_str().to_owned();
        let extra = serde_json::to_string(&entry.extra)?;
        // 网关归属与 provider 请求检查是两个独立事实。请求体上传中断时只有前者，仍应
        // 写出 Key、用户和分组；鉴权前失败时两者都为空，保持现有匿名错误语义。
        let (api_key_name, tenant_id, user_id, username, provider_group_id, provider_group_name) =
            match entry.gateway_attribution {
                Some(attribution) => (
                    Some(attribution.api_key_name),
                    Some(attribution.tenant_id),
                    Some(attribution.user_id),
                    Some(attribution.username),
                    Some(attribution.provider_group_id),
                    Some(attribution.provider_group_name),
                ),
                None => (None, None, None, None, None, None),
            };
        let (model, reasoning, service_tier, fast_mode, is_compaction) = match entry.inspection {
            Some(inspection) => (
                Some(inspection.model),
                inspection.reasoning,
                inspection.service_tier,
                inspection.fast_mode,
                inspection.is_compaction,
            ),
            None => (None, None, None, None, None),
        };

        Ok(Self {
            request_id: entry.request_id,
            provider: entry.provider,
            route: entry.route,
            api_key_name,
            tenant_id,
            user_id,
            username,
            provider_group_id,
            provider_group_name,
            model,
            reasoning,
            service_tier,
            fast_mode,
            is_compaction,
            usage_date: entry
                .request_started_at
                .with_timezone(&service_timezone)
                .date_naive(),
            request_started_at: entry.request_started_at,
            response_started_at: entry.response_started_at,
            response_finished_at: entry.response_finished_at,
            input_tokens: usage.input_tokens,
            cached_input_tokens: usage.cached_input_tokens,
            output_tokens: usage.output_tokens,
            reasoning_output_tokens: usage.reasoning_output_tokens,
            total_tokens: usage.total_tokens,
            status,
            extra,
        })
    }
}

/// 请求日志 writer 的非阻塞派发句柄。
///
/// 日志属于 best-effort 能力：队列满或 worker 退出时丢弃当前日志并记录 error，绝不对
/// 模型响应、资源释放或 SSE 收尾施加反向背压。
#[derive(Clone)]
pub(super) struct RequestLogWriter {
    tx: mpsc::Sender<FinalizedRequestLogEntry>,
}

impl RequestLogWriter {
    pub(super) fn spawn(
        client: Client,
        table: Arc<str>,
        service_timezone: Tz,
    ) -> (Self, JoinHandle<()>) {
        let (tx, rx) = mpsc::channel(REQUEST_LOG_QUEUE_CAPACITY);
        let task = spawn_writer_task(client, table, service_timezone, rx);
        (Self { tx }, task)
    }

    pub(super) fn submit(&self, finalized: FinalizedRequestLogEntry) {
        let request_id = finalized.entry.request_id;
        match self.tx.try_send(finalized) {
            Ok(()) => debug!(request_id = %request_id, "请求日志已投递 ClickHouse writer 队列"),
            Err(error) => error!(
                request_id = %request_id,
                error = %error,
                "请求日志 writer 队列投递失败，当前日志已丢弃"
            ),
        }
    }
}

fn spawn_writer_task(
    client: Client,
    table: Arc<str>,
    service_timezone: Tz,
    mut rx: mpsc::Receiver<FinalizedRequestLogEntry>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        info!(
            clickhouse_table = %table,
            queue_capacity = REQUEST_LOG_QUEUE_CAPACITY,
            batch_max_rows = REQUEST_LOG_BATCH_MAX_ROWS,
            batch_max_estimated_bytes = REQUEST_LOG_BATCH_MAX_ESTIMATED_BYTES,
            batch_flush_interval_ms = REQUEST_LOG_BATCH_FLUSH_INTERVAL_MS,
            service_timezone = %service_timezone,
            "请求日志 ClickHouse writer 已启动"
        );

        let mut batch = RequestLogWriteBatch::new();
        let flush_interval =
            time::sleep(Duration::from_millis(REQUEST_LOG_BATCH_FLUSH_INTERVAL_MS));
        tokio::pin!(flush_interval);

        loop {
            tokio::select! {
                maybe_entry = rx.recv() => {
                    let Some(finalized) = maybe_entry else {
                        break;
                    };

                    let request_id = finalized.entry.request_id;
                    match RequestLogRow::try_from_finalized(finalized, service_timezone) {
                        Ok(row) => {
                            if batch.should_flush_before_push(&row) {
                                flush_request_log_batch(&client, table.as_ref(), &mut batch).await;
                            }
                            batch.push(row);
                            debug!(
                                request_id = %request_id,
                                batch_rows = batch.len(),
                                batch_estimated_bytes = batch.estimated_bytes(),
                                "请求日志已加入 ClickHouse 写入批次"
                            );

                            if batch.should_flush_now() {
                                flush_request_log_batch(&client, table.as_ref(), &mut batch).await;
                            }
                        }
                        Err(error) => error!(
                            request_id = %request_id,
                            error = %error,
                            clickhouse_table = %table,
                            "请求日志 extra 序列化失败，当前日志已丢弃"
                        ),
                    }
                }
                () = &mut flush_interval => {
                    flush_request_log_batch(&client, table.as_ref(), &mut batch).await;
                    flush_interval.as_mut().reset(
                        time::Instant::now()
                            + Duration::from_millis(REQUEST_LOG_BATCH_FLUSH_INTERVAL_MS),
                    );
                }
            }
        }

        flush_request_log_batch(&client, table.as_ref(), &mut batch).await;
        warn!(clickhouse_table = %table, "请求日志 ClickHouse writer 队列已关闭");
    })
}

struct RequestLogWriteBatch {
    rows: Vec<RequestLogRow>,
    estimated_bytes: usize,
}

impl RequestLogWriteBatch {
    fn new() -> Self {
        Self {
            rows: Vec::with_capacity(REQUEST_LOG_BATCH_MAX_ROWS),
            estimated_bytes: 0,
        }
    }

    fn len(&self) -> usize {
        self.rows.len()
    }

    fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    fn estimated_bytes(&self) -> usize {
        self.estimated_bytes
    }

    fn should_flush_before_push(&self, row: &RequestLogRow) -> bool {
        !self.is_empty()
            && (self.rows.len() >= REQUEST_LOG_BATCH_MAX_ROWS
                || self.estimated_bytes.saturating_add(row.estimated_bytes())
                    > REQUEST_LOG_BATCH_MAX_ESTIMATED_BYTES)
    }

    fn should_flush_now(&self) -> bool {
        self.rows.len() >= REQUEST_LOG_BATCH_MAX_ROWS
            || self.estimated_bytes >= REQUEST_LOG_BATCH_MAX_ESTIMATED_BYTES
    }

    fn push(&mut self, row: RequestLogRow) {
        self.estimated_bytes = self.estimated_bytes.saturating_add(row.estimated_bytes());
        self.rows.push(row);
    }

    fn take_rows(&mut self) -> Vec<RequestLogRow> {
        self.estimated_bytes = 0;
        std::mem::replace(
            &mut self.rows,
            Vec::with_capacity(REQUEST_LOG_BATCH_MAX_ROWS),
        )
    }
}

async fn flush_request_log_batch(client: &Client, table: &str, batch: &mut RequestLogWriteBatch) {
    if batch.is_empty() {
        return;
    }

    let row_count = batch.len();
    let estimated_bytes = batch.estimated_bytes();
    let rows = batch.take_rows();
    if let Err(error) = write_request_log_rows(client, table, rows).await {
        error!(
            clickhouse_table = %table,
            row_count,
            estimated_bytes,
            error = %error,
            "请求日志 ClickHouse 批量写入失败，当前批次已丢弃"
        );
    }
}

async fn write_request_log_rows(
    client: &Client,
    table: &str,
    rows: Vec<RequestLogRow>,
) -> AppResult<()> {
    let row_count = rows.len();
    let mut insert = client
        .insert::<RequestLogRow>(table)
        .await
        .map_err(|source| AppError::DbQuery {
            message: format!("创建 ClickHouse 请求日志 insert 失败: {source}"),
        })?;

    for row in &rows {
        insert
            .write(row)
            .await
            .map_err(|source| AppError::DbQuery {
                message: format!("写入 ClickHouse 请求日志行失败: {source}"),
            })?;
    }
    insert.end().await.map_err(|source| AppError::DbQuery {
        message: format!("提交 ClickHouse 请求日志失败: {source}"),
    })?;

    debug!(clickhouse_table = %table, row_count, "请求日志已批量写入 ClickHouse");
    Ok(())
}
