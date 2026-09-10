//! Provider 订阅账号的 Dashboard HTTP 模块。
//!
//! 账号的持久模型和通用维护生命周期由 provider 层统一实现；这里仍按厂商拆分 OAuth
//! 导入协议、厂商专属展示字段及管理端路由，避免把 GPT 与 Claude 的认证细节耦合。

pub(super) mod claude;
pub(super) mod gpt;

/// 在消耗一次性 OAuth state、交换 code 或刷新 token 前提前检查容量。
/// 此处不持锁、不预占名额；最终准入由通用 SQL 创建事务完成。
async fn precheck_resource_capacity(
    state: &crate::state::AppState,
    tenant_id: &str,
    provider: &str,
) -> crate::err::AppResult<()> {
    let mut conn = state.db_conn().await?;
    let tenant = crate::tenant::require_enabled(&mut conn, tenant_id).await?;
    crate::tenant::require_resource_capacity(&mut conn, &tenant, provider, "account").await
}
