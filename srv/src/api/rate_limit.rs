//! 公开接口按来源 IP 独立限流。计数留在当前进程，拒绝时立即返回，不排队。

use std::{net::IpAddr, time::Duration};

use axum::{
    Json,
    http::{Request, StatusCode, header::RETRY_AFTER},
    response::{IntoResponse, Response},
    routing::MethodRouter,
};
use tokio::{task::JoinHandle, time::MissedTickBehavior};
use tower_governor::{
    GovernorError, GovernorLayer, governor::GovernorConfigBuilder, key_extractor::KeyExtractor,
};
use tracing::{debug, error, info, warn};

use super::client_ip::RequestClientIp;
use crate::{config::PublicRateLimitQuota, state::AppState};

#[derive(Clone)]
struct ClientIpKeyExtractor;

impl KeyExtractor for ClientIpKeyExtractor {
    type Key = IpAddr;

    fn name(&self) -> &'static str {
        "trusted client IP"
    }

    fn key_name(&self, key: &Self::Key) -> Option<String> {
        Some(key.to_string())
    }

    fn extract<T>(&self, request: &Request<T>) -> Result<Self::Key, GovernorError> {
        request
            .extensions()
            .get::<RequestClientIp>()
            .and_then(|context| context.client_ip)
            .ok_or(GovernorError::UnableToExtractKey)
    }
}

/// 与 HTTP 服务共同存活；停止服务时取消清理任务，避免 detached 循环。
#[derive(Default)]
pub(crate) struct PublicRateLimitRuntime {
    tasks: Vec<JoinHandle<()>>,
}

impl PublicRateLimitRuntime {
    pub(super) fn limit(
        &mut self,
        route: MethodRouter<AppState>,
        endpoint: &'static str,
        quota: PublicRateLimitQuota,
    ) -> MethodRouter<AppState> {
        let config = GovernorConfigBuilder::default()
            .period(quota.replenish_period())
            .burst_size(quota.burst.get())
            .key_extractor(ClientIpKeyExtractor)
            .finish()
            .expect("限流配置已保证周期和突发容量非零");
        let limiter = config.limiter().clone();
        self.tasks.push(tokio::spawn(async move {
            let interval = Duration::from_secs(60);
            let mut ticks =
                tokio::time::interval_at(tokio::time::Instant::now() + interval, interval);
            ticks.set_missed_tick_behavior(MissedTickBehavior::Skip);
            loop {
                ticks.tick().await;
                // 仅删除额度已完全恢复的 IP，不重置仍受限制的请求来源。
                limiter.retain_recent();
                debug!(
                    endpoint,
                    tracked_ips = limiter.len(),
                    "公开接口限流过期计数清理完成"
                );
            }
        }));
        info!(
            endpoint,
            per_minute = quota.per_minute.get(),
            burst = quota.burst.get(),
            "公开接口 IP 限流已启用，计数范围为当前进程"
        );
        route
            .route_layer(GovernorLayer::new(config).error_handler(move |err| reject(endpoint, err)))
    }
}

impl Drop for PublicRateLimitRuntime {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

fn reject(endpoint: &'static str, err: GovernorError) -> Response {
    match err {
        GovernorError::TooManyRequests { wait_time, .. } => {
            // 库返回向下取整的秒数；向上留足一秒，避免 Retry-After: 0 或过早重试。
            let retry_after = wait_time.saturating_add(1);
            warn!(endpoint, retry_after, "公开接口请求超过 IP 限额");
            let mut response = (StatusCode::TOO_MANY_REQUESTS, Json(serde_json::json!({
                "error": { "code": "public_rate_limited", "message": "请求过于频繁，请稍后再试" }
            }))).into_response();
            response
                .headers_mut()
                .insert(RETRY_AFTER, retry_after.into());
            response
        }
        err => {
            error!(endpoint, error = %err, "公开接口限流无法获取来源 IP，请检查来源 IP 中间件与 ConnectInfo 配置");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": { "code": "internal_error", "message": "服务内部错误" }
                })),
            )
                .into_response()
        }
    }
}
