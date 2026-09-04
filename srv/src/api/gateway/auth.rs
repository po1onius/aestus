use axum::http::{HeaderMap, header::AUTHORIZATION};
use diesel::{
    OptionalExtension,
    sql_types::{Array, BigInt, Bool, Integer, Nullable, Text, Uuid as SqlUuid},
};
use diesel_async::RunQueryDsl;
use tracing::{info, warn};
use uuid::Uuid;

use crate::{
    err::{AppError, AppResult},
    plugin::{self, model::PluginBinding},
    state::AppState,
};

const BEARER_PREFIX: &str = "Bearer ";

/// 一次鉴权查询所需的最小数据库投影。
///
/// Provider 分组和用户使用可空列承接 `LEFT JOIN` 结果。由于数据库不建立外键，API Key
/// 可能因异常数据指向不存在的记录；保留空值能让鉴权逻辑明确记录是哪一段关系失效。
#[derive(diesel::QueryableByName)]
struct GatewayAuthRow {
    #[diesel(sql_type = SqlUuid)]
    api_key_id: Uuid,
    #[diesel(sql_type = Text)]
    tenant_id: String,
    #[diesel(sql_type = Text)]
    api_key_name: String,
    #[diesel(sql_type = Bool)]
    api_key_enabled: bool,
    #[diesel(sql_type = Array<Text>)]
    api_key_allowed_models: Vec<String>,
    #[diesel(sql_type = SqlUuid)]
    group_id: Uuid,
    #[diesel(sql_type = SqlUuid)]
    user_id: Uuid,
    #[diesel(sql_type = Nullable<Bool>)]
    tenant_enabled: Option<bool>,
    #[diesel(sql_type = Nullable<Text>)]
    group_provider: Option<String>,
    #[diesel(sql_type = Nullable<Text>)]
    group_name: Option<String>,
    #[diesel(sql_type = Nullable<Bool>)]
    group_enabled: Option<bool>,
    #[diesel(sql_type = Array<Text>)]
    group_allowed_models: Vec<String>,
    #[diesel(sql_type = Nullable<Text>)]
    username: Option<String>,
    #[diesel(sql_type = Nullable<Text>)]
    user_role: Option<String>,
    #[diesel(sql_type = Nullable<Bool>)]
    user_enabled: Option<bool>,
    #[diesel(sql_type = Nullable<BigInt>)]
    user_quota: Option<i64>,
    #[diesel(sql_type = Nullable<Integer>)]
    user_max_concurrency: Option<i32>,
    #[diesel(sql_type = Bool)]
    group_granted: bool,
    #[diesel(sql_type = Nullable<SqlUuid>)]
    plugin_release_ref: Option<Uuid>,
}

/// 网关后续请求链路使用的最小鉴权上下文。
///
/// 此处刻意不保留原始 API Key、用户密码哈希以及完整数据库模型，既减少热路径中的无效
/// 数据传递，也避免敏感字段被下游模块误用或写入日志。
pub struct GatewayAuth {
    tenant_id: String,
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
    pub fn tenant_id(&self) -> &str {
        &self.tenant_id
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

    // 两层模型白名单使用相关 ARRAY 子查询，避免直接连接两张逐行映射表产生最多
    // 128×128 行笛卡尔积。租户状态和普通用户分组授权也进入同一个 statement snapshot，
    // 使一次请求的全部基础鉴权事实通过单次 PostgreSQL 往返读取。
    let row = diesel::sql_query(
        "SELECT \
             gateway_key.id AS api_key_id, \
             gateway_key.tenant_id, \
             gateway_key.name AS api_key_name, \
             gateway_key.enabled AS api_key_enabled, \
             ARRAY( \
                 SELECT key_model.model_name \
                 FROM api_key_models AS key_model \
                 WHERE key_model.api_key_id = gateway_key.id \
                 ORDER BY key_model.model_name \
             ) AS api_key_allowed_models, \
             gateway_key.group_id, \
             gateway_key.user_id, \
             tenant.enabled AS tenant_enabled, \
             provider_group.provider AS group_provider, \
             provider_group.name AS group_name, \
             provider_group.enabled AS group_enabled, \
             ARRAY( \
                 SELECT group_model.model_name \
                 FROM provider_group_models AS group_model \
                 WHERE group_model.group_id = gateway_key.group_id \
                 ORDER BY group_model.model_name \
             ) AS group_allowed_models, \
             gateway_user.username, \
             gateway_user.role AS user_role, \
             gateway_user.enabled AS user_enabled, \
             gateway_user.quota AS user_quota, \
             gateway_user.max_concurrency AS user_max_concurrency, \
             EXISTS ( \
                 SELECT 1 \
                 FROM tenant_user_group_grants AS group_grant \
                 WHERE group_grant.tenant_id = gateway_key.tenant_id \
                   AND group_grant.user_id = gateway_key.user_id \
                   AND group_grant.group_id = gateway_key.group_id \
             ) AS group_granted, \
             gateway_key.plugin_release_id AS plugin_release_ref \
         FROM api_keys AS gateway_key \
         LEFT JOIN tenants AS tenant \
           ON tenant.id = gateway_key.tenant_id \
         LEFT JOIN provider_groups AS provider_group \
           ON provider_group.id = gateway_key.group_id \
          AND provider_group.tenant_id = gateway_key.tenant_id \
         LEFT JOIN users AS gateway_user \
           ON gateway_user.id = gateway_key.user_id \
          AND gateway_user.tenant_id = gateway_key.tenant_id \
         WHERE gateway_key.api_key = $1 \
         LIMIT 1",
    )
    .bind::<Text, _>(provided_key)
    .get_result::<GatewayAuthRow>(&mut conn)
    .await
    .optional()
    .map_err(|source| AppError::DbQuery {
        message: source.to_string(),
    })?;
    let Some(row) = row else {
        warn!("API Key 未匹配到记录，拒绝请求");
        return Err(AppError::InvalidApiKey);
    };
    let GatewayAuthRow {
        api_key_id,
        tenant_id,
        api_key_name,
        api_key_enabled,
        api_key_allowed_models,
        group_id,
        user_id,
        tenant_enabled,
        group_provider,
        group_name,
        group_enabled,
        group_allowed_models,
        username,
        user_role,
        user_enabled,
        user_quota,
        user_max_concurrency,
        group_granted,
        plugin_release_ref,
    } = row;

    if !api_key_enabled {
        warn!(api_key_id = %api_key_id, api_key_name, "API Key 已禁用，拒绝请求");
        return Err(AppError::DisabledApiKey);
    }
    if api_key_allowed_models.is_empty() {
        warn!(api_key_id = %api_key_id, api_key_name, "API Key 模型白名单为空，拒绝请求");
        return Err(AppError::InvalidApiKey);
    }
    if tenant_enabled != Some(true) {
        warn!(api_key_id = %api_key_id, tenant_id = %tenant_id, "API Key 所属租户不存在或已停用，拒绝请求");
        return Err(AppError::InvalidApiKey);
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

    let (Some(username), Some(user_role), Some(user_enabled), Some(user_quota)) =
        (username, user_role, user_enabled, user_quota)
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

    if user_role == crate::user::USER_ROLE_TENANT_USER {
        if !group_granted {
            warn!(
                api_key_id = %api_key_id,
                user_id = %user_id,
                provider_group_id = %group_id,
                "普通用户的 Provider 分组授权已撤销，拒绝既有 API Key 请求"
            );
            return Err(AppError::InvalidApiKey);
        }
    } else if user_role != crate::user::USER_ROLE_TENANT_OWNER {
        warn!(api_key_id = %api_key_id, user_id = %user_id, role = %user_role, "API Key 所属用户角色不能使用租户 Provider 分组");
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
                plugin::sql::load_binding(&mut conn, tenant_id.clone(), release_id, &group_provider)
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
