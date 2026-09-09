//! 主 Codex 额度窗口内的账号请求日志用量，不推断额外额度项的模型或功能归属。

use chrono::{DateTime, Duration, Utc};
use clickhouse::{Row, sql::Identifier};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};
use uuid::Uuid;

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

/// 仅 owner 专用响应携带；对应主 Codex 额度的 primary / secondary 窗口。
#[derive(Default, Serialize)]
pub(in crate::api::console) struct GptAccountWindowUsage {
    primary: Option<WindowUsage>,
    secondary: Option<WindowUsage>,
}

#[derive(Serialize)]
struct WindowUsage {
    // Token 使用字符串，避免浏览器解析 JSON 时丢失整数精度。
    total_tokens: String,
    users: Vec<UserWindowUsage>,
}

#[derive(Serialize)]
struct UserWindowUsage {
    // 保留未记录用户的分组，避免窗口总量与明细之和不一致。
    user_id: Option<Uuid>,
    username: Option<String>,
    total_tokens: String,
}

#[derive(Deserialize, Row)]
struct UserWindowTokens {
    #[serde(with = "clickhouse::serde::uuid::option")]
    user_id: Option<Uuid>,
    primary_username: Option<String>,
    secondary_username: Option<String>,
    primary_tokens: i64,
    secondary_tokens: i64,
}

/// 调用方必须先完成租户 owner 校验及账号归属校验；按账号统计所有调用者。
pub(in crate::api::console) async fn query_window_usage(
    state: &AppState,
    account: &ProviderAccount,
    quota: &GptAccountQuotaResponse,
) -> AppResult<GptAccountWindowUsage> {
    let Some(snapshot) = quota
        .snapshots
        .iter()
        .find(|snapshot| snapshot.limit_id == DEFAULT_LIMIT_ID)
    else {
        return Ok(GptAccountWindowUsage::default());
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
        "开始统计 GPT 账号额度窗口内各用户的网关已记录 token"
    );
    let Some(earliest_start) = primary_start.into_iter().chain(secondary_start).min() else {
        info!(resource_id = %account.id, "GPT 额度窗口无可查询时间范围，跳过日志聚合");
        return Ok(GptAccountWindowUsage::default());
    };

    // 一次查询按用户汇总两个窗口，再由同一批分组结果计算窗口总量。
    // 不按 status 过滤：失败或中断请求只要记录了 usage，同样计入消费。
    // 不可查询的窗口以 cutoff 绑定空区间，响应仍保留 None，不把未知展示为零。
    // 用户名取各窗口内最新请求的快照；tuple 保留 NULL，同毫秒按 request_id 确定顺序。
    let rows = state
        .clickhouse()
        .query(
            "WITH fromUnixTimestamp64Milli(?, 'UTC') AS primary_start, \
             fromUnixTimestamp64Milli(?, 'UTC') AS secondary_start \
             SELECT user_id, \
             tupleElement(argMaxIf(tuple(username), tuple(request_started_at, request_id), request_started_at >= primary_start), 1) AS primary_username, \
             tupleElement(argMaxIf(tuple(username), tuple(request_started_at, request_id), request_started_at >= secondary_start), 1) AS secondary_username, \
             sumIf(total_tokens, request_started_at >= primary_start) AS primary_tokens, \
             sumIf(total_tokens, request_started_at >= secondary_start) AS secondary_tokens \
             FROM ? \
             WHERE tenant_id = ? AND provider = ? AND resource_id = ? \
             AND request_started_at >= fromUnixTimestamp64Milli(?, 'UTC') \
             AND request_started_at < fromUnixTimestamp64Milli(?, 'UTC') \
             GROUP BY user_id",
        )
        .bind(primary_start.unwrap_or(cutoff).timestamp_millis())
        .bind(secondary_start.unwrap_or(cutoff).timestamp_millis())
        .bind(Identifier(state.config().request_log_table.as_str()))
        .bind(&account.tenant_id)
        .bind(PROVIDER)
        .bind(account.id)
        .bind(earliest_start.timestamp_millis())
        .bind(cutoff.timestamp_millis())
        .fetch_all::<UserWindowTokens>()
        .await
        .map_err(|source| {
            warn!(
                tenant_id = %account.tenant_id,
                resource_id = %account.id,
                earliest_start = %earliest_start,
                cutoff = %cutoff,
                error = %source,
                "查询 GPT 账号窗口用户 token 用量失败"
            );
            AppError::DbQuery {
                message: format!("查询 GPT 账号窗口用户 token 用量失败: {source}"),
            }
        })?;

    let usage = GptAccountWindowUsage {
        primary: primary_start
            .map(|_| summarize_window(&rows, |row| (row.primary_tokens, &row.primary_username))),
        secondary: secondary_start.map(|_| {
            summarize_window(&rows, |row| (row.secondary_tokens, &row.secondary_username))
        }),
    };
    info!(
        resource_id = %account.id,
        user_group_count = rows.len(),
        primary_tokens = ?usage.primary.as_ref().map(|window| window.total_tokens.as_str()),
        secondary_tokens = ?usage.secondary.as_ref().map(|window| window.total_tokens.as_str()),
        "GPT 账号窗口用户 token 用量统计完成"
    );
    Ok(usage)
}

fn summarize_window(
    rows: &[UserWindowTokens],
    select: impl Fn(&UserWindowTokens) -> (i64, &Option<String>),
) -> WindowUsage {
    let total_tokens: i128 = rows.iter().map(|row| i128::from(select(row).0)).sum();
    let mut users: Vec<_> = rows
        .iter()
        .filter_map(|row| {
            let (tokens, username) = select(row);
            (tokens > 0).then_some((tokens, row.user_id, username))
        })
        .collect();
    // 以整数排序，避免字符串字典序或前端 Number 精度影响顺序。
    users.sort_unstable_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    WindowUsage {
        total_tokens: total_tokens.to_string(),
        users: users
            .into_iter()
            .map(|(tokens, user_id, username)| UserWindowUsage {
                user_id,
                username: username.clone(),
                total_tokens: tokens.to_string(),
            })
            .collect(),
    }
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
