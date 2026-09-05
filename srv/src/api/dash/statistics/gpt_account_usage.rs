//! 主 Codex 额度窗口内的账号请求日志用量，不推断额外额度项的模型或功能归属。

use chrono::{DateTime, Duration, Utc};
use clickhouse::{Row, sql::Identifier};
use serde::Deserialize;
use tracing::{info, warn};

use crate::{
    err::{AppError, AppResult},
    provider::{
        credential::ProviderAccount,
        gpt::{
            model::PROVIDER,
            quota::{DEFAULT_LIMIT_ID, GptAccountQuotaResponse, GptQuotaWindow},
        },
    },
    state::AppState,
};

#[derive(Deserialize, Row)]
struct WindowTokenTotals {
    primary_tokens: String,
    secondary_tokens: String,
}

/// 调用方先完成账号额度查看授权；这里按租户和资源统计所有调用者，不按当前用户裁剪。
pub(in crate::api::dash) async fn populate_window_usage(
    state: &AppState,
    account: &ProviderAccount,
    quota: &mut GptAccountQuotaResponse,
) -> AppResult<()> {
    let Some(snapshot) = quota
        .snapshots
        .iter_mut()
        .find(|snapshot| snapshot.limit_id == DEFAULT_LIMIT_ID)
    else {
        return Ok(());
    };
    let cutoff = quota.fetched_at;
    let retention = Duration::days(i64::from(state.config().request_log_retention_days.get()));
    let primary_start = queryable_start(snapshot.primary.as_ref(), cutoff, retention);
    let secondary_start = queryable_start(snapshot.secondary.as_ref(), cutoff, retention);

    info!(
        tenant_id = %account.tenant_id,
        resource_id = %account.id,
        primary_window_start = ?primary_start,
        secondary_window_start = ?secondary_start,
        cutoff = %cutoff,
        "开始统计 GPT 账号额度窗口内的网关已记录 token"
    );
    let Some(earliest_start) = primary_start.into_iter().chain(secondary_start).min() else {
        info!(resource_id = %account.id, "GPT 额度窗口无可查询时间范围，跳过日志聚合");
        return Ok(());
    };

    // 外层时间范围裁剪明细扫描；两个 sumIf 在同一次查询中统计各自窗口。
    // 不按 status 过滤：失败或中断请求只要记录了 usage，同样计入消费。
    // 不可查询的窗口以 cutoff 绑定空区间，响应仍保留 None，不把未知展示为零。
    let totals = state
        .clickhouse()
        .query(
            "SELECT \
             toString(sumIf(total_tokens, request_started_at >= fromUnixTimestamp64Milli(?, 'UTC'))) AS primary_tokens, \
             toString(sumIf(total_tokens, request_started_at >= fromUnixTimestamp64Milli(?, 'UTC'))) AS secondary_tokens \
             FROM ? \
             WHERE tenant_id = ? AND provider = ? AND resource_id = ? \
             AND request_started_at >= fromUnixTimestamp64Milli(?, 'UTC') \
             AND request_started_at < fromUnixTimestamp64Milli(?, 'UTC')",
        )
        .bind(primary_start.unwrap_or(cutoff).timestamp_millis())
        .bind(secondary_start.unwrap_or(cutoff).timestamp_millis())
        .bind(Identifier(state.config().request_log_table.as_str()))
        .bind(&account.tenant_id)
        .bind(PROVIDER)
        .bind(account.id)
        .bind(earliest_start.timestamp_millis())
        .bind(cutoff.timestamp_millis())
        .fetch_one::<WindowTokenTotals>()
        .await
        .map_err(|source| {
            warn!(
                tenant_id = %account.tenant_id,
                resource_id = %account.id,
                earliest_start = %earliest_start,
                cutoff = %cutoff,
                error = %source,
                "查询 GPT 账号窗口 token 用量失败"
            );
            AppError::DbQuery {
                message: format!("查询 GPT 账号窗口 token 用量失败: {source}"),
            }
        })?;

    if let Some(window) = snapshot
        .primary
        .as_mut()
        .filter(|_| primary_start.is_some())
    {
        window.gateway_total_tokens = Some(totals.primary_tokens);
    }
    if let Some(window) = snapshot
        .secondary
        .as_mut()
        .filter(|_| secondary_start.is_some())
    {
        window.gateway_total_tokens = Some(totals.secondary_tokens);
    }
    // 现有响应同时携带 primary 副本和 snapshots，补充数据后保持两份展示结果一致。
    quota.primary = Some(snapshot.clone());
    info!(
        resource_id = %account.id,
        primary_tokens = ?snapshot.primary.as_ref().and_then(|window| window.gateway_total_tokens.as_deref()),
        secondary_tokens = ?snapshot.secondary.as_ref().and_then(|window| window.gateway_total_tokens.as_deref()),
        "GPT 账号窗口 token 用量统计完成"
    );
    Ok(())
}

fn queryable_start(
    window: Option<&GptQuotaWindow>,
    cutoff: DateTime<Utc>,
    retention: Duration,
) -> Option<DateTime<Utc>> {
    let window = window?;
    let start = window.starts_at?;
    let reset = window.resets_at?;
    (start <= cutoff && cutoff < reset && cutoff.signed_duration_since(start) <= retention)
        .then_some(start)
}
