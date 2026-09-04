use std::collections::{BTreeSet, HashMap};

use diesel::prelude::*;
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl};
use tracing::{info, warn};
use uuid::Uuid;

use crate::{
    err::{AppError, AppResult},
    provider::group,
    user::{User, group_access},
};

use super::model::{
    GatewayApiKey, NewGatewayApiKey, NewGatewayApiKeyModel,
    schema::{api_key_models, api_keys},
};

/// API Key 主记录与其逐行模型白名单的领域快照。
pub struct GatewayApiKeyWithModels {
    pub api_key: GatewayApiKey,
    pub allowed_models: Vec<String>,
}

/// 创建 API Key。
///
/// Key 原始值需要供 Dashboard 后续查看与复制，因此直接持久化；日志始终只记录资源
/// 标识和名称，避免凭证意外进入日志系统。
pub async fn create(
    conn: &mut AsyncPgConnection,
    user: &User,
    group_id: Uuid,
    name: String,
    allowed_models: Vec<String>,
    plugin_release_id: Option<Uuid>,
) -> AppResult<GatewayApiKeyWithModels> {
    use self::api_keys::dsl;

    let tenant_id = user.tenant_id.ok_or(AppError::Forbidden)?;
    let user_id = user.id;
    let allowed_models = group::normalize_models(allowed_models)?;

    let result = conn
        .transaction::<GatewayApiKeyWithModels, AppError, _>(async |conn| {
            let provider_group =
                group::require_enabled_for_write(&mut *conn, tenant_id, group_id).await?;
            group_access::require_group_grant(&mut *conn, user, group_id).await?;
            if let Some(plugin_release_id) = plugin_release_id {
                crate::plugin::sql::require_enabled_release_for_provider_write(
                    &mut *conn,
                    tenant_id,
                    plugin_release_id,
                    &provider_group.provider,
                )
                .await?;
            }
            let group_models = group::load_model_names(&mut *conn, group_id).await?;
            ensure_models_within_group(&allowed_models, &group_models, provider_group.id)?;

            let new_api_key = NewGatewayApiKey {
                tenant_id,
                user_id,
                group_id,
                name,
                api_key: generate_api_key(),
                plugin_release_id,
            };
            let api_key = diesel::insert_into(dsl::api_keys)
                .values(&new_api_key)
                .returning(GatewayApiKey::as_returning())
                .get_result::<GatewayApiKey>(&mut *conn)
                .await
                .map_err(|source| match source {
                    diesel::result::Error::DatabaseError(
                        diesel::result::DatabaseErrorKind::UniqueViolation,
                        ref information,
                    ) if information.constraint_name() == Some("idx_api_keys_user_name") => {
                        warn!(
                            user_id = %user_id,
                            api_key_name = %new_api_key.name,
                            "同一用户下的 API Key 名称重复，已拒绝创建"
                        );
                        AppError::BadRequest {
                            message: format!("API Key 名称已存在: {}", new_api_key.name),
                        }
                    }
                    source => AppError::DbQuery {
                        message: source.to_string(),
                    },
                })?;
            let mappings = allowed_models
                .iter()
                .map(|model_name| NewGatewayApiKeyModel {
                    api_key_id: api_key.id,
                    model_name: model_name.clone(),
                })
                .collect::<Vec<_>>();
            diesel::insert_into(api_key_models::table)
                .values(&mappings)
                .execute(&mut *conn)
                .await
                .map_err(|source| AppError::DbQuery {
                    message: source.to_string(),
                })?;
            Ok(GatewayApiKeyWithModels {
                api_key,
                allowed_models: allowed_models.clone(),
            })
        })
        .await;

    let api_key = match result {
        Ok(api_key) => api_key,
        Err(source) => return Err(source),
    };

    info!(api_key_id = %api_key.api_key.id, user_id = %api_key.api_key.user_id, provider_group_id = %api_key.api_key.group_id, api_key_name = %api_key.api_key.name, plugin_release_id = ?api_key.api_key.plugin_release_id, model_count = api_key.allowed_models.len(), allowed_models = ?api_key.allowed_models, "API Key、模型白名单及可选插件套件绑定创建成功");

    Ok(api_key)
}

/// 查询指定用户的 API Key 列表。
pub async fn list_by_user(
    conn: &mut AsyncPgConnection,
    user_id: Uuid,
    limit: i64,
    offset: i64,
) -> AppResult<Vec<GatewayApiKeyWithModels>> {
    use self::api_keys::dsl;

    let api_keys = dsl::api_keys
        .filter(dsl::user_id.eq(user_id))
        .order((dsl::created_at.desc(), dsl::id.desc()))
        .limit(limit)
        .offset(offset)
        .select(GatewayApiKey::as_select())
        .load::<GatewayApiKey>(conn)
        .await
        .map_err(|source| AppError::DbQuery {
            message: source.to_string(),
        })?;
    attach_models(conn, api_keys).await
}

pub async fn find_for_user(
    conn: &mut AsyncPgConnection,
    user_id: Uuid,
    id: Uuid,
) -> AppResult<Option<GatewayApiKey>> {
    use self::api_keys::dsl;

    dsl::api_keys
        .filter(dsl::id.eq(id))
        .filter(dsl::user_id.eq(user_id))
        .select(GatewayApiKey::as_select())
        .first::<GatewayApiKey>(conn)
        .await
        .optional()
        .map_err(|source| AppError::DbQuery {
            message: source.to_string(),
        })
}

/// 永久删除指定用户自己的调用方网关 API Key。
///
/// 项目不使用数据库外键，因此必须在同一事务中先锁定并确认 Key 的用户归属，再显式
/// 删除模型映射和主记录。该操作不触碰 Provider 分组、上游资源或历史请求日志；已经
/// 进入处理流水线的请求继续按其既有生命周期收尾，后续鉴权会立即无法再找到该 Key。
pub async fn delete_for_user(
    conn: &mut AsyncPgConnection,
    user_id: Uuid,
    id: Uuid,
) -> AppResult<GatewayApiKey> {
    use self::api_keys::dsl;

    let deleted = conn
        .transaction::<GatewayApiKey, AppError, _>(async |conn| {
            let current = dsl::api_keys
                .filter(dsl::id.eq(id))
                .filter(dsl::user_id.eq(user_id))
                .for_update()
                .select(GatewayApiKey::as_select())
                .first::<GatewayApiKey>(&mut *conn)
                .await
                .map_err(|source| match source {
                    diesel::result::Error::NotFound => AppError::BadRequest {
                        message: format!("API Key 不存在: {id}"),
                    },
                    source => AppError::DbQuery {
                        message: source.to_string(),
                    },
                })?;

            diesel::delete(api_key_models::table.filter(api_key_models::api_key_id.eq(current.id)))
                .execute(&mut *conn)
                .await
                .map_err(|source| AppError::DbQuery {
                    message: source.to_string(),
                })?;
            let deleted_count = diesel::delete(
                dsl::api_keys
                    .filter(dsl::id.eq(current.id))
                    .filter(dsl::user_id.eq(user_id)),
            )
            .execute(&mut *conn)
            .await
            .map_err(|source| AppError::DbQuery {
                message: source.to_string(),
            })?;
            if deleted_count != 1 {
                return Err(AppError::DbQuery {
                    message: format!("删除 API Key 主记录失败: {}", current.id),
                });
            }
            Ok(current)
        })
        .await?;

    info!(
        api_key_id = %deleted.id,
        user_id = %deleted.user_id,
        provider_group_id = %deleted.group_id,
        api_key_name = %deleted.name,
        was_enabled = deleted.enabled,
        "API Key 及其模型白名单已永久删除；Provider 分组、上游资源和历史日志保持不变"
    );
    Ok(deleted)
}

/// 修改指定用户自己的 API Key 启用状态。
///
/// 网关 Key 的状态只参与请求鉴权，不触碰所属 Provider 分组、上游资源或 maintenance。
/// 重复提交相同状态保持幂等；恢复启用时同步清空 `disabled_at`。
pub async fn update_enabled_for_user(
    conn: &mut AsyncPgConnection,
    user_id: Uuid,
    id: Uuid,
    enabled: bool,
) -> AppResult<GatewayApiKeyWithModels> {
    use self::api_keys::dsl;

    let now = chrono::Utc::now();
    let api_key = diesel::update(
        dsl::api_keys
            .filter(dsl::id.eq(id))
            .filter(dsl::user_id.eq(user_id)),
    )
    .set((
        dsl::enabled.eq(enabled),
        dsl::disabled_at.eq((!enabled).then_some(now)),
        dsl::updated_at.eq(now),
    ))
    .returning(GatewayApiKey::as_returning())
    .get_result::<GatewayApiKey>(conn)
    .await;

    match api_key {
        Ok(api_key) => {
            info!(api_key_id = %api_key.id, user_id = %api_key.user_id, provider_group_id = %api_key.group_id, api_key_name = %api_key.name, enabled = api_key.enabled, "API Key 启用状态已更新；Provider 分组和 maintenance 保持不变");
            let allowed_models = load_model_names(conn, api_key.id).await?;
            Ok(GatewayApiKeyWithModels {
                api_key,
                allowed_models,
            })
        }
        Err(diesel::result::Error::NotFound) => Err(AppError::BadRequest {
            message: format!("API Key 不存在: {id}"),
        }),
        Err(source) => Err(AppError::DbQuery {
            message: source.to_string(),
        }),
    }
}

/// 原子替换指定用户自己的 API Key 模型白名单。
///
/// Key 白名单必须是当前分组白名单的非空子集。后续分组白名单变更不会级联改写 Key；
/// 网关请求会实时同时校验两层集合，因此历史 Key 中暂时超出分组范围的模型也不会获得
/// 调用权限。
pub async fn update_models_for_user(
    conn: &mut AsyncPgConnection,
    user_id: Uuid,
    id: Uuid,
    allowed_models: Vec<String>,
) -> AppResult<GatewayApiKeyWithModels> {
    use self::api_keys::dsl;

    let allowed_models = group::normalize_models(allowed_models)?;
    let api_key = conn
        .transaction::<GatewayApiKey, AppError, _>(async |conn| {
            // 先读取不可变的分组归属，再统一按“分组 -> API Key”顺序加锁。分组删除会
            // 级联删除调用方 Key；固定锁序避免它与白名单更新形成死锁。
            let (tenant_id, group_id) = dsl::api_keys
                .filter(dsl::id.eq(id))
                .filter(dsl::user_id.eq(user_id))
                .select((dsl::tenant_id, dsl::group_id))
                .first::<(Uuid, Uuid)>(&mut *conn)
                .await
                .map_err(|source| match source {
                    diesel::result::Error::NotFound => AppError::BadRequest {
                        message: format!("API Key 不存在: {id}"),
                    },
                    source => AppError::DbQuery {
                        message: source.to_string(),
                    },
                })?;
            group::require_for_update_in_tenant(&mut *conn, tenant_id, group_id).await?;

            let current = dsl::api_keys
                .filter(dsl::id.eq(id))
                .filter(dsl::user_id.eq(user_id))
                .for_update()
                .select(GatewayApiKey::as_select())
                .first::<GatewayApiKey>(&mut *conn)
                .await
                .map_err(|source| match source {
                    diesel::result::Error::NotFound => AppError::BadRequest {
                        message: format!("API Key 不存在: {id}"),
                    },
                    source => AppError::DbQuery {
                        message: source.to_string(),
                    },
                })?;

            // 分组即使被停用也允许编辑 Key；这里只读取它的模型授权边界。分组归属当前
            // 不可修改，仍显式核对锁前投影，防止未来扩展迁移能力时破坏锁序假设。
            if current.group_id != group_id {
                return Err(AppError::BadRequest {
                    message: format!("API Key 所属 Provider 分组已变化，请刷新后重试: {id}"),
                });
            }
            let group_models = group::load_model_names(&mut *conn, current.group_id).await?;
            ensure_models_within_group(&allowed_models, &group_models, current.group_id)?;

            diesel::delete(api_key_models::table.filter(api_key_models::api_key_id.eq(id)))
                .execute(&mut *conn)
                .await
                .map_err(|source| AppError::DbQuery {
                    message: source.to_string(),
                })?;
            let mappings = allowed_models
                .iter()
                .map(|model_name| NewGatewayApiKeyModel {
                    api_key_id: id,
                    model_name: model_name.clone(),
                })
                .collect::<Vec<_>>();
            diesel::insert_into(api_key_models::table)
                .values(&mappings)
                .execute(&mut *conn)
                .await
                .map_err(|source| AppError::DbQuery {
                    message: source.to_string(),
                })?;

            diesel::update(
                dsl::api_keys
                    .filter(dsl::id.eq(id))
                    .filter(dsl::user_id.eq(user_id)),
            )
            .set(dsl::updated_at.eq(chrono::Utc::now()))
            .returning(GatewayApiKey::as_returning())
            .get_result::<GatewayApiKey>(&mut *conn)
            .await
            .map_err(|source| AppError::DbQuery {
                message: source.to_string(),
            })
        })
        .await?;

    info!(
        api_key_id = %api_key.id,
        user_id = %api_key.user_id,
        provider_group_id = %api_key.group_id,
        api_key_name = %api_key.name,
        model_count = allowed_models.len(),
        allowed_models = ?allowed_models,
        "API Key 模型白名单已更新；Provider 分组白名单和 maintenance 保持不变"
    );
    Ok(GatewayApiKeyWithModels {
        api_key,
        allowed_models,
    })
}

fn ensure_models_within_group(
    allowed_models: &[String],
    group_models: &[String],
    group_id: Uuid,
) -> AppResult<()> {
    let group_model_set = group_models
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let disallowed_models = allowed_models
        .iter()
        .filter(|model| !group_model_set.contains(model.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    if disallowed_models.is_empty() {
        return Ok(());
    }
    warn!(
        provider_group_id = %group_id,
        disallowed_models = ?disallowed_models,
        "API Key 白名单包含分组未授权模型，拒绝写入"
    );
    Err(AppError::BadRequest {
        message: format!(
            "模型不在 Provider 分组允许范围内: {}",
            disallowed_models.join(", ")
        ),
    })
}

/// 修改指定用户自己的 API Key 插件绑定。
///
/// API Key 的 Provider 由其分组决定，因此非空绑定必须在同一事务内验证 release 仍处于
/// 启用状态且 Provider 一致。传入 `None` 始终表示解除绑定，不依赖原插件当前是否启用。
pub async fn update_plugin_for_user(
    conn: &mut AsyncPgConnection,
    user_id: Uuid,
    id: Uuid,
    plugin_release_id: Option<Uuid>,
) -> AppResult<GatewayApiKeyWithModels> {
    use self::api_keys::dsl;

    let api_key = conn
        .transaction::<GatewayApiKey, AppError, _>(async |conn| {
            let current = dsl::api_keys
                .filter(dsl::id.eq(id))
                .filter(dsl::user_id.eq(user_id))
                .select(GatewayApiKey::as_select())
                .first::<GatewayApiKey>(&mut *conn)
                .await
                .optional()
                .map_err(|source| AppError::DbQuery {
                    message: source.to_string(),
                })?
                .ok_or_else(|| AppError::BadRequest {
                    message: format!("API Key 不存在: {id}"),
                })?;

            if let Some(release_id) = plugin_release_id {
                let provider_group = group::find_by_id(&mut *conn, current.group_id)
                    .await?
                    .ok_or_else(|| AppError::DbQuery {
                        message: format!(
                            "API Key 关联的 Provider 分组不存在: {}",
                            current.group_id
                        ),
                    })?;
                crate::plugin::sql::require_enabled_release_for_provider_write(
                    &mut *conn,
                    current.tenant_id,
                    release_id,
                    &provider_group.provider,
                )
                .await?;
            }

            diesel::update(
                dsl::api_keys
                    .filter(dsl::id.eq(id))
                    .filter(dsl::user_id.eq(user_id)),
            )
            .set((
                dsl::plugin_release_id.eq(plugin_release_id),
                dsl::updated_at.eq(chrono::Utc::now()),
            ))
            .returning(GatewayApiKey::as_returning())
            .get_result::<GatewayApiKey>(&mut *conn)
            .await
            .map_err(|source| AppError::DbQuery {
                message: source.to_string(),
            })
        })
        .await?;

    let allowed_models = load_model_names(conn, api_key.id).await?;
    info!(
        api_key_id = %api_key.id,
        user_id = %api_key.user_id,
        provider_group_id = %api_key.group_id,
        api_key_name = %api_key.name,
        plugin_release_id = ?api_key.plugin_release_id,
        "API Key 插件绑定已更新"
    );
    Ok(GatewayApiKeyWithModels {
        api_key,
        allowed_models,
    })
}

/// 批量加载 API Key 模型映射，列表分页只产生一次额外查询，不随 Key 数量产生 N+1。
async fn load_models_by_api_key_ids(
    conn: &mut AsyncPgConnection,
    api_key_ids: &[Uuid],
) -> AppResult<HashMap<Uuid, Vec<String>>> {
    use api_key_models::dsl;

    if api_key_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = dsl::api_key_models
        .filter(dsl::api_key_id.eq_any(api_key_ids))
        .order((dsl::api_key_id.asc(), dsl::model_name.asc()))
        .select((dsl::api_key_id, dsl::model_name))
        .load::<(Uuid, String)>(conn)
        .await
        .map_err(|source| AppError::DbQuery {
            message: source.to_string(),
        })?;
    let mut models_by_api_key = HashMap::<Uuid, Vec<String>>::new();
    for (api_key_id, model_name) in rows {
        models_by_api_key
            .entry(api_key_id)
            .or_default()
            .push(model_name);
    }
    Ok(models_by_api_key)
}

async fn load_model_names(
    conn: &mut AsyncPgConnection,
    api_key_id: Uuid,
) -> AppResult<Vec<String>> {
    load_models_by_api_key_ids(conn, &[api_key_id])
        .await?
        .remove(&api_key_id)
        .ok_or_else(|| AppError::DbQuery {
            message: format!("API Key 缺少模型白名单映射: {api_key_id}"),
        })
}

async fn attach_models(
    conn: &mut AsyncPgConnection,
    api_keys: Vec<GatewayApiKey>,
) -> AppResult<Vec<GatewayApiKeyWithModels>> {
    let api_key_ids = api_keys
        .iter()
        .map(|api_key| api_key.id)
        .collect::<Vec<_>>();
    let mut models_by_api_key = load_models_by_api_key_ids(conn, &api_key_ids).await?;
    api_keys
        .into_iter()
        .map(|api_key| {
            let allowed_models =
                models_by_api_key
                    .remove(&api_key.id)
                    .ok_or_else(|| AppError::DbQuery {
                        message: format!("API Key 缺少模型白名单映射: {}", api_key.id),
                    })?;
            Ok(GatewayApiKeyWithModels {
                api_key,
                allowed_models,
            })
        })
        .collect()
}

fn generate_api_key() -> String {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use rand::RngExt;

    let mut random_bytes = [0_u8; 32];
    rand::rng().fill(&mut random_bytes);
    format!("oclg_{}", URL_SAFE_NO_PAD.encode(random_bytes))
}
