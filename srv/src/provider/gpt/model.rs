use serde::{Deserialize, Serialize};

pub const PROVIDER: &str = "gpt";
pub const PLAN_TYPE_UNKNOWN: &str = "unknown";

/// GPT 账号在通用 `specific` 字段中的强类型内容。
///
/// `chatgpt_account_id` 是请求上下文而不是账号主键，允许不同凭证记录使用相同值。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GptAccountSpecific {
    #[serde(default)]
    pub chatgpt_account_id: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default = "default_plan_type")]
    pub plan_type: String,
    #[serde(default)]
    pub chatgpt_account_is_fedramp: bool,
}

fn default_plan_type() -> String {
    PLAN_TYPE_UNKNOWN.to_owned()
}

/// GPT OAuth 账号参与上游请求时所需的最小上下文。
///
/// 邮箱和套餐只用于管理、维护与额度展示，不进入 Redis 请求热路径。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GptAccountRequestContext {
    #[serde(default)]
    pub chatgpt_account_id: Option<String>,
    #[serde(default)]
    pub chatgpt_account_is_fedramp: bool,
}
