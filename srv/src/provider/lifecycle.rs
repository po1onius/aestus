use tokio::task::JoinHandle;
use tracing::info;

use crate::{
    err::AppResult,
    provider::{
        maintenance::{self, MaintenanceProvider},
        scheduler,
    },
    state::AppState,
};

/// 启动单个 provider 的完整运行生命周期。
///
/// 生命周期编排与 maintenance、scheduler 同级：这里固定完成遗留负载清理、PostgreSQL
/// runtime 重投影和维护循环启动，避免 maintenance 与 scheduler 互相调用。新增 provider
/// 只需在组合根登记 `MaintenanceProvider` 实现。
pub(super) async fn start_provider<P: MaintenanceProvider>(
    state: &AppState,
) -> AppResult<JoinHandle<()>> {
    info!(provider = P::NAME, "开始初始化 provider 运行生命周期");
    scheduler::reset_loads(state, P::NAME).await?;
    let ready_runtime_count = maintenance::bootstrap_provider_runtime::<P>(state).await?;
    let task = maintenance::spawn_maintenance_loop::<P>(state.clone());
    info!(
        provider = P::NAME,
        ready_runtime_count, "provider 运行生命周期初始化完成，maintenance 循环已启动"
    );
    Ok(task)
}
