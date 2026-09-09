use tokio::task::JoinHandle;
use tracing::info;

use crate::{
    err::AppResult,
    provider::maintenance::{self, MaintenanceProvider},
    state::AppState,
};

/// 启动单个 provider 的完整运行生命周期。
///
/// 生命周期编排与 maintenance、scheduler 同级：这里完成 PostgreSQL runtime 增量同步和
/// 维护循环启动，保留其他实例正在使用的共享调度状态。新增 provider
/// 只需在组合根登记 `MaintenanceProvider` 实现。
pub(super) async fn start_provider<P: MaintenanceProvider>(
    state: &AppState,
) -> AppResult<JoinHandle<()>> {
    info!(provider = P::NAME, "开始初始化 provider 运行生命周期");
    let published_ready_count = maintenance::bootstrap_provider_runtime::<P>(state).await?;
    let task = maintenance::spawn_maintenance_loop::<P>(state.clone());
    info!(
        provider = P::NAME,
        published_ready_count, "provider 运行生命周期初始化完成，maintenance 循环已启动"
    );
    Ok(task)
}
