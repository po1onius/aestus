use axum::http::{HeaderMap, header::AUTHORIZATION};
use diesel::prelude::*;
use diesel_async::RunQueryDsl;
use tracing::{info, warn};
use uuid::Uuid;

use crate::{
    err::{AppError, AppResult},
    gateway_key::schema::{api_key_models, api_keys},
    plugin::{self, model::PluginBinding},
    provider::group::{self, schema::provider_groups},
    state::AppState,
    tenant,
    user::schema::users,
};

// 四张表分别定义在不同领域模块中。项目不使用数据库外键，因此这里显式声明鉴权查询
// 需要组合的表；关联条件则全部写在下方查询的 `ON` 子句里。
diesel::allow_tables_to_appear_in_same_query!(api_keys, provider_groups);
diesel::allow_tables_to_appear_in_same_query!(api_key_models, provider_groups);
diesel::allow_tables_to_appear_in_same_query!(users, provider_groups);
diesel::allow_tables_to_appear_in_same_query!(api_keys, users);
diesel::allow_tables_to_appear_in_same_query!(api_key_models, users);

const BEARER_PREFIX: &str = "Bearer ";

/// 一次鉴权查询所需的最小数据库投影。
///
/// Provider 分组和用户使用可空列承接 `LEFT JOIN` 结果。由于数据库不建立外键，API Key
/// 可能因异常数据指向不存在的记录；保留空值能让鉴权逻辑明确记录是哪一段关系失效。
#[derive(Queryable)]
struct GatewayAuthRow {
    api_key_id: Uuid,
    tenant_id: Uuid,
    api_key_name: String,
    api_key_enabled: bool,
    allowed_model: String,
    group_id: Uuid,
    user_id: Uuid,
    group_provider: Option<String>,
    group_name: Option<String>,
    group_enabled: Option<bool>,
    username: Option<String>,
    user_enabled: Option<bool>,
    user_quota: Option<i64>,
    user_max_concurrency: Option<i32>,
    plugin_release_ref: Option<Uuid>,
}

/// 网关后续请求链路使用的最小鉴权上下文。
///
/// 此处刻意不保留原始 API Key、用户密码哈希以及完整数据库模型，既减少热路径中的无效
/// 数据传递，也避免敏感字段被下游模块误用或写入日志。
pub struct GatewayAuth {
    tenant_id: Uuid,
    api_key_id: Uuid,
    api_key_name: String,
    api_key_allowed_models: Vec<String>,
    group_allowed_models: Vec<String>,
    user_id: Uuid,
    username: String,
    max_concurrency: Option<i32>,
    group_id: Uuid,
    group_name: String,
    plugin: Option<PluginBinding>,
}

impl GatewayAuth {
    pub fn tenant_id(&self) -> Uuid {
        self.tenant_id
    }

    pub fn api_key_id(&self) -> Uuid {
        self.api_key_id
    }

    pub fn api_key_name(&self) -> &str {
        &self.api_key_name
    }

    pub fn user_id(&self) -> Uuid {
        self.user_id
    }

    pub fn username(&self) -> &str {
        &self.username
    }

    pub fn max_concurrency(&self) -> Option<i32> {
        self.max_concurrency
    }

    pub fn group_id(&self) -> Uuid {
        self.group_id
    }

    pub fn group_name(&self) -> &str {
        &self.group_name
    }

    pub fn plugin(&self) -> Option<&PluginBinding> {
        self.plugin.as_ref()
    }

    /// API Key 和 Provider 分组各自维护独立非空白名单；请求模型必须同时精确命中
    /// 两层集合。配置变化只在后续请求鉴权时生效，不级联修改另一层数据或 maintenance。
    fn model_authorization(&self, model: &str) -> (bool, bool) {
        let api_key_allows = self
            .api_key_allowed_models
            .iter()
            .any(|allowed_model| allowed_model == model);
        let group_allows = self
            .group_allowed_models
            .iter()
            .any(|allowed_model| allowed_model == model);
        (api_key_allows, group_allows)
    }
}

/// 校验网关 API Key，兼容 OpenAI Bearer 和 Anthropic `x-api-key` 两种客户端格式。
///
/// 该阶段通过一次数据库往返完成 API Key、Provider 分组和用户鉴权，但不读取请求体、
/// 不校验模型、不扣减额度，目的是在昂贵的 body 缓存和 JSON 解析前先拦截无效请求。
pub async fn authenticate_gateway_key(
    state: &AppState,
    headers: &HeaderMap,
    expected_provider: &str,
    plugin_endpoint: bool,
) -> AppResult<GatewayAuth> {
    let provided_key = extract_gateway_key(headers)?;
    let mut conn = state.db_conn().await?;

    // 分组和用户使用 LEFT JOIN：没有数据库外键兜底时，可以区分“Key 不存在”和
    // “Key 存在但分组/用户关联丢失”，便于从日志定位数据一致性问题。
    // API Key 模型白名单逐行存储，随基础身份查询一次读出；分组模型在确认分组存在、
    // 启用且 Provider 匹配后再读取，避免无效凭证触发额外查询。
    let rows = api_keys::table
        .inner_join(api_key_models::table.on(api_key_models::api_key_id.eq(api_keys::id)))
        .left_join(
            provider_groups::table.on(provider_groups::id
                .eq(api_keys::group_id)
                .and(provider_groups::tenant_id.eq(api_keys::tenant_id))),
        )
        .left_join(
            users::table.on(users::id
                .eq(api_keys::user_id)
                .and(users::tenant_id.eq(api_keys::tenant_id.nullable()))),
        )
        .filter(api_keys::api_key.eq(provided_key))
        .select((
            api_keys::id,
            api_keys::tenant_id,
            api_keys::name,
            api_keys::enabled,
            api_key_models::model_name,
            api_keys::group_id,
            api_keys::user_id,
            provider_groups::provider.nullable(),
            provider_groups::name.nullable(),
            provider_groups::enabled.nullable(),
            users::username.nullable(),
            users::enabled.nullable(),
            users::quota.nullable(),
            users::max_concurrency.nullable(),
            api_keys::plugin_release_id,
        ))
        .load::<GatewayAuthRow>(&mut conn)
        .await;

    let mut rows = match rows {
        Ok(rows) if !rows.is_empty() => rows,
        Ok(_) | Err(diesel::result::Error::NotFound) => {
            warn!("API Key 未匹配到记录，拒绝请求");
            return Err(AppError::InvalidApiKey);
        }
        Err(source) => {
            return Err(AppError::DbQuery {
                message: source.to_string(),
            });
        }
    };
    let first = rows.swap_remove(0);
    let mut api_key_allowed_models = rows
        .into_iter()
        .map(|row| row.allowed_model)
        .collect::<Vec<_>>();
    api_key_allowed_models.push(first.allowed_model.clone());
    api_key_allowed_models.sort_unstable();

    let GatewayAuthRow {
        api_key_id,
        tenant_id,
        api_key_name,
        api_key_enabled,
        allowed_model: _,
        group_id,
        user_id,
        group_provider,
        group_name,
        group_enabled,
        username,
        user_enabled,
        user_quota,
        user_max_concurrency,
        plugin_release_ref,
    } = first;

    if !api_key_enabled {
        warn!(api_key_id = %api_key_id, api_key_name, "API Key 已禁用，拒绝请求");
        return Err(AppError::DisabledApiKey);
    }

    match tenant::require_enabled(&mut conn, tenant_id).await {
        Ok(_) => {}
        Err(AppError::Forbidden) => {
            warn!(api_key_id = %api_key_id, tenant_id = %tenant_id, "API Key 所属租户不存在或已停用，拒绝请求");
            return Err(AppError::InvalidApiKey);
        }
        Err(source) => return Err(source),
    }

    let (Some(group_provider), Some(group_name), Some(group_enabled)) =
        (group_provider, group_name, group_enabled)
    else {
        warn!(
            api_key_id = %api_key_id,
            api_key_name,
            provider_group_id = %group_id,
            "API Key 指向的 Provider 分组不存在，拒绝请求"
        );
        return Err(AppError::InvalidApiKey);
    };

    if !group_enabled {
        warn!(api_key_id = %api_key_id, provider_group_id = %group_id, provider_group_name = %group_name, "API Key 所属 Provider 分组已归档，拒绝请求");
        return Err(AppError::GatewayKeyGroupUnavailable);
    }
    if group_provider != expected_provider {
        warn!(
            api_key_id = %api_key_id,
            provider_group_id = %group_id,
            key_provider = %group_provider,
            requested_provider = expected_provider,
            "API Key 分组 provider 与请求接口不匹配，拒绝请求"
        );
        return Err(AppError::GatewayKeyProviderMismatch {
            key_provider: group_provider,
            requested_provider: expected_provider.to_owned(),
        });
    }

    // 分组模型独立逐行存储。若直接与 Key 模型连接，最多会产生 128×128 行笛卡尔积；
    // 因此在基础身份与分组状态通过后追加一次小查询，以有界成本取得第二层授权集合。
    let group_allowed_models = group::load_model_names(&mut conn, group_id).await?;

    let (Some(username), Some(user_enabled), Some(user_quota)) =
        (username, user_enabled, user_quota)
    else {
        warn!(
            api_key_id = %api_key_id,
            api_key_name,
            user_id = %user_id,
            "API Key 指向的用户不存在，拒绝请求"
        );
        return Err(AppError::InvalidApiKey);
    };

    if !user_enabled {
        warn!(
            api_key_id = %api_key_id,
            user_id = %user_id,
            username,
            "API Key 所属用户已禁用，拒绝请求"
        );
        return Err(AppError::InvalidApiKey);
    }

    if user_quota <= 0 {
        warn!(
            api_key_id = %api_key_id,
            user_id = %user_id,
            username,
            quota = user_quota,
            "API Key 所属用户 token 额度不足，拒绝请求"
        );
        return Err(AppError::UserQuotaExceeded);
    }

    // 插件 artifact 使用独立逐行表，避免让模型白名单与三个插槽做笛卡尔积。只有绑定了
    // 套件的模型端点才追加一次小查询，并在这里完整验证 provider、启停状态和 ABI。
    let plugin = if !plugin_endpoint {
        // 绑定只对 provider 的模型生成端点生效。其他端点不因为插件被禁用、删除或 ABI
        // 变化而失败，确保插件不会间接扩大到 count_tokens 等接口。
        None
    } else {
        match plugin_release_ref {
            None => None,
            Some(release_id) => Some(
                plugin::sql::load_binding(&mut conn, tenant_id, release_id, &group_provider)
                    .await
                    .map_err(|error| {
                        warn!(api_key_id = %api_key_id, plugin_release_id = %release_id, error = %error, "API Key 插件套件绑定不可用，拒绝请求");
                        AppError::Plugin {
                            message: format!("API Key 插件套件绑定不可用: release_id={release_id}"),
                        }
                    })?,
            ),
        }
    };

    info!(
        api_key_id = %api_key_id,
        api_key_name,
        user_id = %user_id,
        username,
        provider_group_id = %group_id,
        provider_group_name = %group_name,
        provider = %group_provider,
        quota = user_quota,
        max_concurrency = ?user_max_concurrency,
        plugin_release_id = ?plugin.as_ref().map(|binding| binding.release_id),
        plugin_artifact_count = plugin.as_ref().map_or(0, |binding| binding.artifacts.len()),
        api_key_model_count = api_key_allowed_models.len(),
        group_model_count = group_allowed_models.len(),
        "API Key、Provider 分组、用户与可选插件套件已完成 header 级鉴权"
    );

    Ok(GatewayAuth {
        tenant_id,
        api_key_id,
        api_key_name,
        api_key_allowed_models,
        group_allowed_models,
        user_id,
        username,
        max_concurrency: user_max_concurrency,
        group_id,
        group_name,
        plugin,
    })
}

/// 在请求体解析完成后做 payload 级授权。
///
/// 模型白名单依赖请求体里的 model 字段。用户 token 额度只在 provider 明确解析到
/// usage 后发布非阻塞请求事件，由后台额度消费者处理；这里仅做授权检查，不预扣。
pub fn authorize_gateway_payload(auth: &GatewayAuth, model: Option<&str>) -> AppResult<()> {
    if let Some(model) = model {
        let (api_key_allows, group_allows) = auth.model_authorization(model);
        if !api_key_allows || !group_allows {
            warn!(
                api_key_id = %auth.api_key_id,
                api_key_name = %auth.api_key_name,
                provider_group_id = %auth.group_id,
                provider_group_name = %auth.group_name,
                model,
                api_key_allows,
                group_allows,
                "请求模型未同时通过 API Key 与 Provider 分组白名单，拒绝请求"
            );
            return Err(AppError::ModelNotAllowed {
                model: model.to_owned(),
            });
        }
    }

    info!(
        api_key_id = %auth.api_key_id,
        api_key_name = %auth.api_key_name,
        user_id = %auth.user_id,
        username = %auth.username,
        "API Key payload 级鉴权通过，等待 provider 返回确定 token usage 后发布额度事件"
    );

    Ok(())
}

fn extract_gateway_key(headers: &HeaderMap) -> AppResult<&str> {
    if let Some(raw_value) = headers.get(AUTHORIZATION) {
        return raw_value
            .to_str()
            .map_err(|_| AppError::MissingApiKey)?
            .strip_prefix(BEARER_PREFIX)
            .map(str::trim)
            .filter(|key| !key.is_empty())
            .ok_or(AppError::MissingApiKey);
    }

    let key = headers
        .get("x-api-key")
        .ok_or(AppError::MissingApiKey)?
        .to_str()
        .map_err(|_| AppError::MissingApiKey)?
        .trim();
    (!key.is_empty())
        .then_some(key)
        .ok_or(AppError::MissingApiKey)
}
