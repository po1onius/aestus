pub mod claude;
pub mod credential;
pub mod gpt;
pub mod group;
mod lifecycle;
pub mod maintenance;
pub mod oauth;
pub mod protocol;
pub mod proxy;
mod request_header;
pub mod resource;
mod response_logging;
pub mod runtime;
pub mod scheduler;
pub mod service;
pub mod sql;

use tokio::task::JoinHandle;
use tracing::info;

use self::{
    claude::maintenance::ClaudeMaintenance, gpt::maintenance::GptMaintenance,
    lifecycle::start_provider,
};
use crate::{err::AppResult, state::AppState};

/// 服务进程持有的全部 provider 后台任务。
///
/// `JoinHandle` 被直接丢弃时 Tokio 会让任务继续 detached 运行，因此这里集中持有并在
/// 服务生命周期结束时主动 abort，确保启动中途失败或 HTTP 服务退出后不遗留维护任务。
pub struct ProviderTasks {
    maintenance_loops: Vec<JoinHandle<()>>,
}

impl ProviderTasks {
    fn new() -> Self {
        Self {
            maintenance_loops: Vec::new(),
        }
    }

    fn push(&mut self, task: JoinHandle<()>) {
        self.maintenance_loops.push(task);
    }
}

impl Drop for ProviderTasks {
    fn drop(&mut self) {
        let task_count = self.maintenance_loops.len();
        for task in &self.maintenance_loops {
            task.abort();
        }
        info!(task_count, "provider maintenance 后台任务已停止");
    }
}

/// 具体 provider 的统一组合根。
///
/// 通用 maintenance 只掌握单个 provider 的启动步骤；系统启用了哪些 provider 统一在
/// 这里登记，避免通用层反向依赖 GPT、Claude 等具体实现。
pub async fn start(state: &AppState) -> AppResult<ProviderTasks> {
    let mut tasks = ProviderTasks::new();
    tasks.push(start_provider::<GptMaintenance>(state).await?);
    tasks.push(start_provider::<ClaudeMaintenance>(state).await?);
    info!(
        provider_count = tasks.maintenance_loops.len(),
        "全部 provider 运行生命周期已启动"
    );
    Ok(tasks)
}
