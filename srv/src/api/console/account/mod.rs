//! Provider 订阅账号的 Dashboard HTTP 模块。
//!
//! 账号的持久模型和通用维护生命周期由 provider 层统一实现；这里仍按厂商拆分 OAuth
//! 导入协议、厂商专属展示字段及管理端路由，避免把 GPT 与 Claude 的认证细节耦合。

pub(super) mod claude;
pub(super) mod gpt;
