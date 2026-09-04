use std::collections::{BTreeSet, HashMap};

use chrono::{DateTime, Utc};
use diesel::prelude::*;
use diesel::{dsl::now as db_now, pg::expression::extensions::IntervalDsl, sql_types::Timestamptz};
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl};
use serde::Serialize;
use tracing::{info, warn};
use uuid::Uuid;

use crate::{
    err::{AppError, AppResult},
    gateway_key::schema::{api_key_models, api_keys},
    provider::{
        credential::{
            ProviderAccount, ProviderApiKey,
            schema::{provider_accounts, provider_api_keys},
        },
        resource::UpstreamResourceKind,
    },
    user::group_access::schema::{tenant_user_group_grants, tenant_user_group_permissions},
};

const MAX_GROUP_NAME_BYTES: usize = 128;
const MAX_ALLOWED_MODELS: usize = 128;
const MAX_MODEL_NAME_BYTES: usize = 256;

diesel::define_sql_function! {
    #[sql_name = "GREATEST"]
    fn greatest_group_projection(left: Timestamptz, right: Timestamptz) -> Timestamptz;
}

/// Provider 分组是账号、官方 API Key 与调用方网关 Key 的共同调度边界。
///
/// 数据库不使用外键；所有关联写入都必须先通过本模块确认分组存在、启用且 provider
/// 一致。provider 创建后不可修改，避免一个 group_id 在生命周期中改变协议语义。
pub mod schema {
    diesel::table! {
        provider_groups (id) {
            id -> Uuid,
            tenant_id -> Uuid,
            provider -> Text,
            name -> Text,
            enabled -> Bool,
            created_at -> Timestamptz,
            updated_at -> Timestamptz,
            disabled_at -> Nullable<Timestamptz>,
        }
    }

    diesel::table! {
        provider_group_models (group_id, model_name) {
            group_id -> Uuid,
            model_name -> Text,
            created_at -> Timestamptz,
        }
    }

    diesel::allow_tables_to_appear_in_same_query!(provider_groups, provider_group_models,);
}

#[derive(Debug, Clone, Queryable, Selectable, Serialize)]
#[diesel(table_name = schema::provider_groups)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct ProviderGroup {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub provider: String,
    pub name: String,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub disabled_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Insertable)]
#[diesel(table_name = schema::provider_groups)]
struct NewProviderGroup {
    tenant_id: Uuid,
    provider: String,
    name: String,
}

#[derive(Debug, Queryable, Selectable)]
#[diesel(table_name = schema::provider_group_models)]
#[diesel(check_for_backend(diesel::pg::Pg))]
struct ProviderGroupModel {
    group_id: Uuid,
    model_name: String,
}

#[derive(Debug, Insertable)]
#[diesel(table_name = schema::provider_group_models)]
struct NewProviderGroupModel {
    group_id: Uuid,
    model_name: String,
}

/// Dashboard 分组选项需要同时携带可选模型，调用方不需要了解底层逐行映射结构。
#[derive(Debug, Clone, Serialize)]
pub struct ProviderGroupWithModels {
    #[serde(flatten)]
    pub group: ProviderGroup,
    pub allowed_models: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProviderGroupCounts {
    pub account_count: i64,
    pub upstream_api_key_count: i64,
    pub gateway_api_key_count: i64,
    pub enabled_gateway_api_key_count: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProviderGroupSummary {
    #[serde(flatten)]
    pub group: ProviderGroup,
    pub allowed_models: Vec<String>,
    pub counts: ProviderGroupCounts,
}

/// 创建分组弹窗使用的未分组资源投影。只返回可识别资源所需的非敏感字段，绝不把长期
/// token 或官方 API Key 暴露给前端。
#[derive(Debug, Clone, Serialize)]
pub struct UnassignedProviderResource {
    pub id: Uuid,
    pub resource_type: UpstreamResourceKind,
    pub display_name: String,
    pub detail: String,
}

/// 分组和资源归属在同一 PostgreSQL 事务中提交；HTTP 层随后使用返回的完整资源快照
/// 更新 Redis runtime，不需要再次查询或猜测本次实际占用了哪些记录。
pub struct CreatedProviderGroup {
    pub group: ProviderGroupWithModels,
    pub accounts: Vec<ProviderAccount>,
    pub api_keys: Vec<ProviderApiKey>,
}

/// 删除分组事务提交后的完整影响快照。
///
/// 上游账号和官方 API Key 只解除分组归属，凭证记录继续保留；调用方网关 Key 无法脱离
/// 分组独立存在，因此与其模型映射一起永久删除。HTTP 层使用资源快照把 PostgreSQL 的
/// 最新事实同步到 Redis runtime，并向管理员返回精确的删除统计。
pub struct DeletedProviderGroup {
    pub group: ProviderGroup,
    pub accounts: Vec<ProviderAccount>,
    pub upstream_api_keys: Vec<ProviderApiKey>,
    pub deleted_gateway_api_key_count: usize,
}

pub fn normalize_name(name: String) -> AppResult<String> {
    let name = name.trim().to_owned();
    if name.is_empty() {
        return Err(AppError::BadRequest {
            message: "分组名称不能为空".to_owned(),
        });
    }
    if name.len() > MAX_GROUP_NAME_BYTES {
        return Err(AppError::BadRequest {
            message: format!("分组名称不能超过 {MAX_GROUP_NAME_BYTES} 字节"),
        });
    }
    Ok(name)
}

/// 统一规范化分组模型与 API Key 白名单，确保两个写入入口使用完全相同的集合语义。
pub fn normalize_models(models: Vec<String>) -> AppResult<Vec<String>> {
    if models.len() > MAX_ALLOWED_MODELS {
        return Err(AppError::BadRequest {
            message: format!("模型列表最多包含 {MAX_ALLOWED_MODELS} 个模型"),
        });
    }

    let models = models
        .into_iter()
        .map(|model| model.trim().to_owned())
        .filter(|model| !model.is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if models.is_empty() {
        return Err(AppError::BadRequest {
            message: "模型列表至少要包含一个有效模型名".to_owned(),
        });
    }
    if let Some(model) = models
        .iter()
        .find(|model| model.len() > MAX_MODEL_NAME_BYTES)
    {
        return Err(AppError::BadRequest {
            message: format!("模型名过长: {model:?}，最多 {MAX_MODEL_NAME_BYTES} 字节"),
        });
    }
    Ok(models)
}

pub fn ensure_supported_provider(provider: &str) -> AppResult<()> {
    if matches!(
        provider,
        crate::provider::gpt::model::PROVIDER | crate::provider::claude::model::PROVIDER
    ) {
        return Ok(());
    }
    Err(AppError::BadRequest {
        message: format!("不支持为 provider 创建分组: {provider}"),
    })
}

pub async fn list_enabled(
    conn: &mut AsyncPgConnection,
    tenant_id: Uuid,
) -> AppResult<Vec<ProviderGroupWithModels>> {
    use schema::provider_groups::dsl;

    let groups = dsl::provider_groups
        .filter(dsl::enabled.eq(true))
        .filter(dsl::tenant_id.eq(tenant_id))
        .order((dsl::provider.asc(), dsl::name.asc(), dsl::id.asc()))
        .select(ProviderGroup::as_select())
        .load(conn)
        .await
        .map_err(db_error)?;
    attach_models(conn, groups).await
}

pub async fn list_summaries(
    conn: &mut AsyncPgConnection,
    tenant_id: Uuid,
    provider: Option<&str>,
) -> AppResult<Vec<ProviderGroupSummary>> {
    use schema::provider_groups::dsl;

    if let Some(provider) = provider {
        ensure_supported_provider(provider)?;
    }
    let groups = match provider {
        Some(provider) => dsl::provider_groups
            .filter(dsl::tenant_id.eq(tenant_id))
            .filter(dsl::provider.eq(provider))
            .order((dsl::created_at.asc(), dsl::id.asc()))
            .select(ProviderGroup::as_select())
            .load(conn)
            .await
            .map_err(db_error)?,
        None => dsl::provider_groups
            .filter(dsl::tenant_id.eq(tenant_id))
            .order((dsl::provider.asc(), dsl::created_at.asc(), dsl::id.asc()))
            .select(ProviderGroup::as_select())
            .load(conn)
            .await
            .map_err(db_error)?,
    };

    let group_ids = groups.iter().map(|group| group.id).collect::<Vec<_>>();
    let mut models_by_group = load_models_by_group_ids(conn, &group_ids).await?;
    let mut summaries = Vec::with_capacity(groups.len());
    for group in groups {
        let counts = load_counts(conn, group.id).await?;
        let allowed_models = take_required_models(&mut models_by_group, group.id, &group.name)?;
        summaries.push(ProviderGroupSummary {
            group,
            allowed_models,
            counts,
        });
    }
    Ok(summaries)
}

pub async fn list_unassigned_resources(
    conn: &mut AsyncPgConnection,
    tenant_id: Uuid,
    provider: &str,
) -> AppResult<Vec<UnassignedProviderResource>> {
    ensure_supported_provider(provider)?;

    let accounts = provider_accounts::table
        .filter(provider_accounts::tenant_id.eq(tenant_id))
        .filter(provider_accounts::provider.eq(provider))
        .filter(provider_accounts::group_id.is_null())
        .order((
            provider_accounts::created_at.asc(),
            provider_accounts::id.asc(),
        ))
        .select((provider_accounts::id, provider_accounts::client_id))
        .load::<(Uuid, String)>(conn)
        .await
        .map_err(db_error)?;
    let api_keys = provider_api_keys::table
        .filter(provider_api_keys::tenant_id.eq(tenant_id))
        .filter(provider_api_keys::provider.eq(provider))
        .filter(provider_api_keys::group_id.is_null())
        .order((
            provider_api_keys::created_at.asc(),
            provider_api_keys::id.asc(),
        ))
        .select((provider_api_keys::id, provider_api_keys::base_url))
        .load::<(Uuid, String)>(conn)
        .await
        .map_err(db_error)?;

    let mut resources = Vec::with_capacity(accounts.len() + api_keys.len());
    resources.extend(
        accounts
            .into_iter()
            .map(|(id, client_id)| UnassignedProviderResource {
                id,
                resource_type: UpstreamResourceKind::Account,
                display_name: "OAuth 账号".to_owned(),
                detail: format!("client_id: {client_id}"),
            }),
    );
    resources.extend(
        api_keys
            .into_iter()
            .map(|(id, base_url)| UnassignedProviderResource {
                id,
                resource_type: UpstreamResourceKind::ApiKey,
                display_name: "官方 API Key".to_owned(),
                detail: base_url,
            }),
    );
    Ok(resources)
}

pub async fn find_by_id(
    conn: &mut AsyncPgConnection,
    id: Uuid,
) -> AppResult<Option<ProviderGroup>> {
    use schema::provider_groups::dsl;

    match dsl::provider_groups
        .filter(dsl::id.eq(id))
        .select(ProviderGroup::as_select())
        .first(conn)
        .await
    {
        Ok(group) => Ok(Some(group)),
        Err(diesel::result::Error::NotFound) => Ok(None),
        Err(source) => Err(db_error(source)),
    }
}

pub async fn find_by_ids(
    conn: &mut AsyncPgConnection,
    tenant_id: Uuid,
    ids: &[Uuid],
) -> AppResult<HashMap<Uuid, ProviderGroup>> {
    use schema::provider_groups::dsl;

    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let groups = dsl::provider_groups
        .filter(dsl::tenant_id.eq(tenant_id))
        .filter(dsl::id.eq_any(ids))
        .select(ProviderGroup::as_select())
        .load::<ProviderGroup>(conn)
        .await
        .map_err(db_error)?;
    Ok(groups.into_iter().map(|group| (group.id, group)).collect())
}

/// 批量读取分组模型映射，供列表接口和 API Key 创建事务复用。
pub async fn load_models_by_group_ids(
    conn: &mut AsyncPgConnection,
    group_ids: &[Uuid],
) -> AppResult<HashMap<Uuid, Vec<String>>> {
    use schema::provider_group_models::dsl;

    if group_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = dsl::provider_group_models
        .filter(dsl::group_id.eq_any(group_ids))
        .order((dsl::group_id.asc(), dsl::model_name.asc()))
        .select(ProviderGroupModel::as_select())
        .load::<ProviderGroupModel>(conn)
        .await
        .map_err(db_error)?;
    let mut models_by_group = HashMap::<Uuid, Vec<String>>::new();
    for row in rows {
        models_by_group
            .entry(row.group_id)
            .or_default()
            .push(row.model_name);
    }
    Ok(models_by_group)
}

pub async fn load_model_names(
    conn: &mut AsyncPgConnection,
    group_id: Uuid,
) -> AppResult<Vec<String>> {
    load_models_by_group_ids(conn, &[group_id])
        .await?
        .remove(&group_id)
        .ok_or_else(|| AppError::DbQuery {
            message: format!("Provider 分组缺少模型映射: {group_id}"),
        })
}

pub async fn with_models(
    conn: &mut AsyncPgConnection,
    group: ProviderGroup,
) -> AppResult<ProviderGroupWithModels> {
    let allowed_models = load_model_names(conn, group.id).await?;
    Ok(ProviderGroupWithModels {
        group,
        allowed_models,
    })
}

async fn attach_models(
    conn: &mut AsyncPgConnection,
    groups: Vec<ProviderGroup>,
) -> AppResult<Vec<ProviderGroupWithModels>> {
    let group_ids = groups.iter().map(|group| group.id).collect::<Vec<_>>();
    let mut models_by_group = load_models_by_group_ids(conn, &group_ids).await?;
    groups
        .into_iter()
        .map(|group| {
            let allowed_models = take_required_models(&mut models_by_group, group.id, &group.name)?;
            Ok(ProviderGroupWithModels {
                group,
                allowed_models,
            })
        })
        .collect()
}

fn take_required_models(
    models_by_group: &mut HashMap<Uuid, Vec<String>>,
    group_id: Uuid,
    group_name: &str,
) -> AppResult<Vec<String>> {
    models_by_group
        .remove(&group_id)
        .ok_or_else(|| AppError::DbQuery {
            message: format!("Provider 分组缺少模型映射: id={group_id}, name={group_name}"),
        })
}

pub async fn require_for_update_in_tenant(
    conn: &mut AsyncPgConnection,
    tenant_id: Uuid,
    id: Uuid,
) -> AppResult<ProviderGroup> {
    use schema::provider_groups::dsl;

    dsl::provider_groups
        .filter(dsl::id.eq(id))
        .filter(dsl::tenant_id.eq(tenant_id))
        .for_update()
        .select(ProviderGroup::as_select())
        .first::<ProviderGroup>(conn)
        .await
        .map_err(|source| match source {
            diesel::result::Error::NotFound => AppError::BadRequest {
                message: format!("Provider 分组不存在: {id}"),
            },
            source => db_error(source),
        })
}

pub async fn require_enabled_for_write(
    conn: &mut AsyncPgConnection,
    tenant_id: Uuid,
    id: Uuid,
) -> AppResult<ProviderGroup> {
    let group = require_for_update_in_tenant(conn, tenant_id, id).await?;
    if !group.enabled {
        warn!(
            provider = %group.provider,
            provider_group_id = %group.id,
            provider_group_name = %group.name,
            "Provider 分组已归档，拒绝分配新资源"
        );
        return Err(AppError::BadRequest {
            message: format!("Provider 分组已归档，不能再分配资源: {}", group.name),
        });
    }
    Ok(group)
}

pub async fn require_enabled_for_provider_write(
    conn: &mut AsyncPgConnection,
    tenant_id: Uuid,
    id: Uuid,
    provider: &str,
) -> AppResult<ProviderGroup> {
    let group = require_enabled_for_write(conn, tenant_id, id).await?;
    if group.provider != provider {
        warn!(
            provider_group_id = %group.id,
            provider_group_name = %group.name,
            actual_provider = %group.provider,
            expected_provider = provider,
            "Provider 分组与资源 provider 不匹配，拒绝写入"
        );
        return Err(AppError::BadRequest {
            message: format!(
                "Provider 分组不匹配: group_id={id}, expected={provider}, actual={}",
                group.provider
            ),
        });
    }
    Ok(group)
}

pub async fn create(
    conn: &mut AsyncPgConnection,
    tenant_id: Uuid,
    provider: String,
    name: String,
    models: Vec<String>,
    account_ids: Vec<Uuid>,
    api_key_ids: Vec<Uuid>,
) -> AppResult<CreatedProviderGroup> {
    let provider = provider.trim().to_ascii_lowercase();
    ensure_supported_provider(&provider)?;
    let name = normalize_name(name)?;
    let models = normalize_models(models)?;
    let account_ids = account_ids.into_iter().collect::<BTreeSet<_>>();
    let api_key_ids = api_key_ids.into_iter().collect::<BTreeSet<_>>();
    if account_ids.is_empty() && api_key_ids.is_empty() {
        return Err(AppError::BadRequest {
            message: "创建 Provider 分组时至少要选择一个未分组账号或官方 API Key".to_owned(),
        });
    }

    let (group, accounts, api_keys) = conn
        .transaction::<
            (ProviderGroup, Vec<ProviderAccount>, Vec<ProviderApiKey>),
            AppError,
            _,
        >(async |conn| {
            use schema::{provider_group_models, provider_groups};

            let result = diesel::insert_into(provider_groups::table)
                .values(&NewProviderGroup {
                    tenant_id,
                    provider: provider.clone(),
                    name: name.clone(),
                })
                .returning(ProviderGroup::as_returning())
                .get_result(&mut *conn)
                .await;
            let group = map_group_write_error(result, &provider, &name)?;
            let mappings = models
                .iter()
                .map(|model_name| NewProviderGroupModel {
                    group_id: group.id,
                    model_name: model_name.clone(),
                })
                .collect::<Vec<_>>();
            diesel::insert_into(provider_group_models::table)
                .values(&mappings)
                .execute(&mut *conn)
                .await
                .map_err(db_error)?;

            // UPDATE ... WHERE group_id IS NULL 同时承担占用和并发校验：两个管理员若选择
            // 同一资源，只会有一个事务成功，另一个事务因返回数量不足而整体回滚。
            let accounts = if account_ids.is_empty() {
                Vec::new()
            } else {
                diesel::update(
                    provider_accounts::table
                        .filter(provider_accounts::tenant_id.eq(tenant_id))
                        .filter(provider_accounts::provider.eq(&provider))
                        .filter(provider_accounts::id.eq_any(&account_ids))
                        .filter(provider_accounts::group_id.is_null()),
                )
                .set((
                    provider_accounts::group_id.eq(Some(group.id)),
                    // 归属变化必须严格推进 Redis 投影版本。使用数据库时钟并与旧值取
                    // GREATEST，避免应用与 PostgreSQL 时钟偏差或并发事务造成版本倒退。
                    provider_accounts::updated_at.eq(greatest_group_projection(
                        db_now,
                        provider_accounts::updated_at + 1.microseconds(),
                    )),
                ))
                .returning(ProviderAccount::as_returning())
                .load::<ProviderAccount>(&mut *conn)
                .await
                .map_err(db_error)?
            };
            if accounts.len() != account_ids.len() {
                return Err(AppError::BadRequest {
                    message: "部分账号不存在、Provider 不匹配或已被其他分组占用，请刷新后重试"
                        .to_owned(),
                });
            }

            let api_keys = if api_key_ids.is_empty() {
                Vec::new()
            } else {
                diesel::update(
                    provider_api_keys::table
                        .filter(provider_api_keys::tenant_id.eq(tenant_id))
                        .filter(provider_api_keys::provider.eq(&provider))
                        .filter(provider_api_keys::id.eq_any(&api_key_ids))
                        .filter(provider_api_keys::group_id.is_null()),
                )
                .set((
                    provider_api_keys::group_id.eq(Some(group.id)),
                    provider_api_keys::updated_at.eq(greatest_group_projection(
                        db_now,
                        provider_api_keys::updated_at + 1.microseconds(),
                    )),
                ))
                .returning(ProviderApiKey::as_returning())
                .load::<ProviderApiKey>(&mut *conn)
                .await
                .map_err(db_error)?
            };
            if api_keys.len() != api_key_ids.len() {
                return Err(AppError::BadRequest {
                    message: "部分官方 API Key 不存在、Provider 不匹配或已被其他分组占用，请刷新后重试"
                        .to_owned(),
                });
            }

            Ok((group, accounts, api_keys))
        })
        .await?;
    info!(provider, provider_group_id = %group.id, provider_group_name = %group.name, model_count = models.len(), allowed_models = ?models, account_count = accounts.len(), upstream_api_key_count = api_keys.len(), "Provider 分组、模型映射及初始资源归属创建成功");
    Ok(CreatedProviderGroup {
        group: ProviderGroupWithModels {
            group,
            allowed_models: models,
        },
        accounts,
        api_keys,
    })
}

pub async fn rename(
    conn: &mut AsyncPgConnection,
    tenant_id: Uuid,
    id: Uuid,
    name: String,
) -> AppResult<ProviderGroup> {
    use schema::provider_groups::dsl;

    let name = normalize_name(name)?;
    let current = find_by_id(conn, id)
        .await?
        .ok_or_else(|| AppError::BadRequest {
            message: format!("Provider 分组不存在: {id}"),
        })?;
    if current.tenant_id != tenant_id {
        return Err(AppError::BadRequest {
            message: format!("Provider 分组不存在: {id}"),
        });
    }
    let result = diesel::update(
        dsl::provider_groups
            .filter(dsl::id.eq(id))
            .filter(dsl::tenant_id.eq(tenant_id)),
    )
    .set((dsl::name.eq(&name), dsl::updated_at.eq(Utc::now())))
    .returning(ProviderGroup::as_returning())
    .get_result(conn)
    .await;
    let group = map_group_write_error(result, &current.provider, &name)?;
    info!(provider = %group.provider, provider_group_id = %group.id, provider_group_name = %group.name, "Provider 分组名称更新成功");
    Ok(group)
}

/// 原子替换 Provider 分组的模型白名单。
///
/// 分组模型只参与网关鉴权，不改变其下调用方 Key 的独立白名单，也不触碰上游资源的
/// Redis runtime。请求时必须同时通过分组与 Key 两层模型授权，因此这里无需级联更新
/// `api_key_models`，也无需通知 maintenance。
pub async fn update_models(
    conn: &mut AsyncPgConnection,
    tenant_id: Uuid,
    id: Uuid,
    models: Vec<String>,
) -> AppResult<ProviderGroupWithModels> {
    let models = normalize_models(models)?;
    let group = conn
        .transaction::<ProviderGroup, AppError, _>(async |conn| {
            use schema::{provider_group_models, provider_groups};

            // 锁定主记录，使并发的启停、模型替换以及 Key 白名单子集校验按同一分组串行
            // 落库。项目不使用外键，因此还必须先显式确认分组存在再改逐行映射。
            let current = provider_groups::table
                .filter(provider_groups::id.eq(id))
                .filter(provider_groups::tenant_id.eq(tenant_id))
                .for_update()
                .select(ProviderGroup::as_select())
                .first::<ProviderGroup>(&mut *conn)
                .await
                .map_err(|source| match source {
                    diesel::result::Error::NotFound => AppError::BadRequest {
                        message: format!("Provider 分组不存在: {id}"),
                    },
                    source => db_error(source),
                })?;

            diesel::delete(
                provider_group_models::table.filter(provider_group_models::group_id.eq(current.id)),
            )
            .execute(&mut *conn)
            .await
            .map_err(db_error)?;
            let mappings = models
                .iter()
                .map(|model_name| NewProviderGroupModel {
                    group_id: current.id,
                    model_name: model_name.clone(),
                })
                .collect::<Vec<_>>();
            diesel::insert_into(provider_group_models::table)
                .values(&mappings)
                .execute(&mut *conn)
                .await
                .map_err(db_error)?;

            diesel::update(provider_groups::table.filter(provider_groups::id.eq(current.id)))
                .set(provider_groups::updated_at.eq(Utc::now()))
                .returning(ProviderGroup::as_returning())
                .get_result::<ProviderGroup>(&mut *conn)
                .await
                .map_err(db_error)
        })
        .await?;

    info!(
        provider = %group.provider,
        provider_group_id = %group.id,
        provider_group_name = %group.name,
        model_count = models.len(),
        allowed_models = ?models,
        "Provider 分组模型白名单已更新；调用方 Key 白名单和上游 runtime 保持不变"
    );
    Ok(ProviderGroupWithModels {
        group,
        allowed_models: models,
    })
}

pub async fn update_enabled(
    conn: &mut AsyncPgConnection,
    tenant_id: Uuid,
    id: Uuid,
    enabled: bool,
) -> AppResult<ProviderGroup> {
    conn.transaction::<ProviderGroup, AppError, _>(async |conn| {
            use schema::provider_groups::dsl;

            let group = dsl::provider_groups
                .filter(dsl::id.eq(id))
                .filter(dsl::tenant_id.eq(tenant_id))
                .for_update()
                .select(ProviderGroup::as_select())
                .first::<ProviderGroup>(&mut *conn)
                .await
                .map_err(|source| match source {
                    diesel::result::Error::NotFound => AppError::BadRequest {
                        message: format!("Provider 分组不存在: {id}"),
                    },
                    source => db_error(source),
                })?;
            if group.enabled == enabled {
                return Ok(group);
            }

            let now = Utc::now();
            diesel::update(dsl::provider_groups.filter(dsl::id.eq(id)))
                .set((
                    dsl::enabled.eq(enabled),
                    dsl::disabled_at.eq((!enabled).then_some(now)),
                    dsl::updated_at.eq(now),
                ))
                .returning(ProviderGroup::as_returning())
                .get_result(&mut *conn)
                .await
                .map_err(db_error)
    })
    .await
    .inspect(|group| {
        info!(provider = %group.provider, provider_group_id = %group.id, provider_group_name = %group.name, enabled = group.enabled, "Provider 分组状态更新成功");
    })
}

/// 原子删除 Provider 分组及所有不能脱离分组存在的调用方配置。
///
/// 删除不要求先停用分组。事务首先锁定分组，使并发创建网关 Key、修改分组模型和删除
/// 串行化；随后把上游账号和官方 API Key 释放为未分组状态，删除调用方网关 Key及两层
/// 模型映射，最后删除分组主记录。项目不使用数据库外键，因此每一种关联都必须在这里
/// 显式处理并校验受影响行数。
pub async fn delete(
    conn: &mut AsyncPgConnection,
    tenant_id: Uuid,
    id: Uuid,
) -> AppResult<DeletedProviderGroup> {
    let deleted = conn
        .transaction::<DeletedProviderGroup, AppError, _>(async |conn| {
            use schema::{provider_group_models, provider_groups};

            let group = require_for_update_in_tenant(&mut *conn, tenant_id, id).await?;

            let accounts = diesel::update(
                provider_accounts::table.filter(provider_accounts::group_id.eq(group.id)),
            )
            .set((
                provider_accounts::group_id.eq(None::<Uuid>),
                provider_accounts::updated_at.eq(greatest_group_projection(
                    db_now,
                    provider_accounts::updated_at + 1.microseconds(),
                )),
            ))
            .returning(ProviderAccount::as_returning())
            .load::<ProviderAccount>(&mut *conn)
            .await
            .map_err(db_error)?;

            let upstream_api_keys = diesel::update(
                provider_api_keys::table.filter(provider_api_keys::group_id.eq(group.id)),
            )
            .set((
                provider_api_keys::group_id.eq(None::<Uuid>),
                provider_api_keys::updated_at.eq(greatest_group_projection(
                    db_now,
                    provider_api_keys::updated_at + 1.microseconds(),
                )),
            ))
            .returning(ProviderApiKey::as_returning())
            .load::<ProviderApiKey>(&mut *conn)
            .await
            .map_err(db_error)?;

            // 网关 Key 模型映射没有数据库外键，必须先取得并锁定主记录 ID，再显式删除
            // 映射。所有会同时锁定分组和网关 Key 的写路径统一使用“分组 -> Key”顺序，
            // 避免删除与模型白名单更新形成反向锁序。
            let gateway_api_key_ids = api_keys::table
                .filter(api_keys::group_id.eq(group.id))
                .order(api_keys::id.asc())
                .for_update()
                .select(api_keys::id)
                .load::<Uuid>(&mut *conn)
                .await
                .map_err(db_error)?;
            if !gateway_api_key_ids.is_empty() {
                diesel::delete(
                    api_key_models::table
                        .filter(api_key_models::api_key_id.eq_any(&gateway_api_key_ids)),
                )
                .execute(&mut *conn)
                .await
                .map_err(db_error)?;
            }
            let deleted_gateway_api_key_count =
                diesel::delete(api_keys::table.filter(api_keys::group_id.eq(group.id)))
                    .execute(&mut *conn)
                    .await
                    .map_err(db_error)?;
            if deleted_gateway_api_key_count != gateway_api_key_ids.len() {
                return Err(AppError::DbQuery {
                    message: format!(
                        "删除 Provider 分组时网关 Key 数量发生并发变化: group_id={}, expected={}, actual={deleted_gateway_api_key_count}",
                        group.id,
                        gateway_api_key_ids.len(),
                    ),
                });
            }

            // 分组授权没有数据库外键，删除分组时必须先清理权限明细，再清理授权主记录，
            // 避免普通用户保留指向已删除分组的授权事实。
            diesel::delete(
                tenant_user_group_permissions::table
                    .filter(tenant_user_group_permissions::group_id.eq(group.id)),
            )
            .execute(&mut *conn)
            .await
            .map_err(db_error)?;
            diesel::delete(
                tenant_user_group_grants::table
                    .filter(tenant_user_group_grants::group_id.eq(group.id)),
            )
            .execute(&mut *conn)
            .await
            .map_err(db_error)?;

            diesel::delete(
                provider_group_models::table
                    .filter(provider_group_models::group_id.eq(group.id)),
            )
            .execute(&mut *conn)
            .await
            .map_err(db_error)?;
            let deleted_group_count = diesel::delete(
                provider_groups::table.filter(provider_groups::id.eq(group.id)),
            )
            .execute(&mut *conn)
            .await
            .map_err(db_error)?;
            if deleted_group_count != 1 {
                return Err(AppError::DbQuery {
                    message: format!("删除 Provider 分组主记录失败: {}", group.id),
                });
            }

            Ok(DeletedProviderGroup {
                group,
                accounts,
                upstream_api_keys,
                deleted_gateway_api_key_count,
            })
        })
        .await?;

    info!(
        provider = %deleted.group.provider,
        provider_group_id = %deleted.group.id,
        provider_group_name = %deleted.group.name,
        provider_group_was_enabled = deleted.group.enabled,
        released_account_count = deleted.accounts.len(),
        released_upstream_api_key_count = deleted.upstream_api_keys.len(),
        deleted_gateway_api_key_count = deleted.deleted_gateway_api_key_count,
        "Provider 分组及关联调用方配置已删除，上游资源已释放为未分组状态"
    );
    Ok(deleted)
}

async fn load_counts(
    conn: &mut AsyncPgConnection,
    group_id: Uuid,
) -> AppResult<ProviderGroupCounts> {
    let account_count = provider_accounts::table
        .filter(provider_accounts::group_id.eq(group_id))
        .count()
        .get_result(conn)
        .await
        .map_err(db_error)?;
    let upstream_api_key_count = provider_api_keys::table
        .filter(provider_api_keys::group_id.eq(group_id))
        .count()
        .get_result(conn)
        .await
        .map_err(db_error)?;
    let gateway_api_key_count = api_keys::table
        .filter(api_keys::group_id.eq(group_id))
        .count()
        .get_result(conn)
        .await
        .map_err(db_error)?;
    let enabled_gateway_api_key_count = api_keys::table
        .filter(api_keys::group_id.eq(group_id))
        .filter(api_keys::enabled.eq(true))
        .count()
        .get_result(conn)
        .await
        .map_err(db_error)?;
    Ok(ProviderGroupCounts {
        account_count,
        upstream_api_key_count,
        gateway_api_key_count,
        enabled_gateway_api_key_count,
    })
}

fn map_group_write_error(
    result: Result<ProviderGroup, diesel::result::Error>,
    provider: &str,
    name: &str,
) -> AppResult<ProviderGroup> {
    match result {
        Ok(group) => Ok(group),
        Err(diesel::result::Error::DatabaseError(
            diesel::result::DatabaseErrorKind::UniqueViolation,
            information,
        )) if information.constraint_name() == Some("uq_provider_groups_tenant_provider_name") => {
            warn!(
                provider,
                provider_group_name = name,
                "同一 provider 下的分组名称重复，已拒绝写入"
            );
            Err(AppError::BadRequest {
                message: format!("{provider} 下已存在同名分组: {name}"),
            })
        }
        Err(source) => Err(db_error(source)),
    }
}

fn db_error(source: diesel::result::Error) -> AppError {
    AppError::DbQuery {
        message: source.to_string(),
    }
}
