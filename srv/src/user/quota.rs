use chrono::Utc;
use diesel::{dsl::case_when, prelude::*};
use diesel_async::RunQueryDsl;
use tracing::info;
use uuid::Uuid;

use crate::{
    err::{AppError, AppResult},
    infra::db::{self, DbPool},
    request::events::TokenUsage,
};

use super::model::{User, schema::users};

/// JavaScript `number` 能精确表示的最大整数。Dashboard JSON 直接以数字返回额度，因此
/// 持久层也必须使用同一上限，避免浏览器收到已经发生舍入的额度。
pub const MAX_USER_QUOTA: i64 = 9_007_199_254_740_991;

/// 对确定 token usage 直接扣减用户额度。
///
/// 用户额度是网关的附属能力，这里只在一条 UPDATE 中同步维护余额和累计消耗，不维护
/// 额外扣费账本。调用方已经保证同一条请求流只在收尾时提交一次确定 usage。
pub async fn deduct_quota(
    db_pool: &DbPool,
    request_id: Uuid,
    user_id: Uuid,
    api_key_id: Uuid,
    usage: TokenUsage,
) -> AppResult<i64> {
    use self::users::dsl as users_dsl;

    // quota worker 只持有完成本任务所需的数据库连接池，不反向依赖包含 worker 句柄的
    // 整个 AppState，避免后台任务参数形成隐式服务定位器。
    let mut conn = db::get_connection(db_pool).await?;
    let token_cost = usage.total_tokens.max(0);
    // BIGINT 的容量远大于实际 token 使用规模；仍在表达式中做封顶，避免长期运行后累计
    // 计数溢出导致本次余额扣减也被数据库整体回滚。
    let consumed_tokens_ceiling = i64::MAX - token_cost;
    let updated = diesel::update(users_dsl::users.filter(users_dsl::id.eq(user_id)))
        .set((
            users_dsl::quota.eq(case_when(
                users_dsl::quota.ge(token_cost),
                users_dsl::quota - token_cost,
            )
            .otherwise(0_i64)),
            users_dsl::consumed_tokens.eq(case_when(
                users_dsl::consumed_tokens.le(consumed_tokens_ceiling),
                users_dsl::consumed_tokens + token_cost,
            )
            .otherwise(i64::MAX)),
            users_dsl::updated_at.eq(Utc::now()),
        ))
        .returning(User::as_returning())
        .get_result::<User>(&mut conn)
        .await
        .map_err(|source| AppError::DbQuery {
            message: source.to_string(),
        })?;

    info!(
        request_id = %request_id,
        user_id = %user_id,
        username = %updated.username,
        api_key_id = %api_key_id,
        token_cost,
        quota_after = updated.quota,
        consumed_tokens_after = updated.consumed_tokens,
        "用户 token 额度已按确定 usage 扣减"
    );

    Ok(updated.quota)
}

pub(super) fn validate_user_quota(quota: i64) -> AppResult<()> {
    if !(0..=MAX_USER_QUOTA).contains(&quota) {
        return Err(AppError::BadRequest {
            message: format!("用户额度必须在 0 到 {MAX_USER_QUOTA} 之间"),
        });
    }
    Ok(())
}
