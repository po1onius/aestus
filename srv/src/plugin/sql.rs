//! 公共资源使用 NULL 归属。读取可见范围为“公共 + 当前租户”，写入只匹配精确归属。
//! 传入 None 表示平台公共作用域，绝不表示忽略租户过滤。跨租户删除只从已授权插件的依赖展开。
use std::collections::{HashMap, HashSet};

use diesel::prelude::*;
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl};
use serde::Serialize;
use tracing::{info, warn};
use uuid::Uuid;

use super::model::{
    NewPlugin, NewPluginSuite, PluginSuiteSummary, PluginSummary,
    schema::{plugin_suites, plugins},
};
use crate::{
    err::{AppError, AppResult},
    gateway_key::schema::api_keys,
};

pub async fn create_plugin(
    conn: &mut AsyncPgConnection,
    input: NewPlugin,
) -> AppResult<PluginSummary> {
    let plugin = diesel::insert_into(plugins::table)
        .values(input)
        .returning(PluginSummary::as_returning())
        .get_result::<PluginSummary>(conn)
        .await
        .map_err(map_write_error)?;
    info!(tenant_id = ?plugin.tenant_id, plugin_id = %plugin.id, provider = %plugin.provider,
        slot = %plugin.slot, wasm_size = plugin.wasm_size, wasm_sha256 = %plugin.wasm_sha256, "WASM 插件已上传");
    Ok(plugin)
}

pub async fn list_plugins(
    conn: &mut AsyncPgConnection,
    tenant_id: Option<String>,
) -> AppResult<Vec<PluginSummary>> {
    Ok(plugins::table
        .filter(
            plugins::tenant_id
                .is_null()
                .or(plugins::tenant_id.eq(tenant_id)),
        )
        .order((plugins::created_at.desc(), plugins::id.desc()))
        .select(PluginSummary::as_select())
        .load(conn)
        .await?)
}

/// 所有套件写入均先按 UUID 顺序锁定引用插件。删除插件持有同一行锁后才扫描套件，
/// 因此不会漏掉并发创建的组合；套件创建后没有修改组合的入口。
pub async fn create_suite(
    conn: &mut AsyncPgConnection,
    input: NewPluginSuite,
) -> AppResult<PluginSuiteSummary> {
    let suite = conn.transaction::<PluginSuiteSummary, AppError, _>(async |conn| {
        let slots = [
            ("request", input.request_plugin_id),
            ("buffered_response", input.buffered_response_plugin_id),
            ("stream_response", input.stream_response_plugin_id),
        ];
        let ids = slots.iter().filter_map(|(_, id)| *id).collect::<Vec<_>>();
        if ids.is_empty() {
            return Err(AppError::BadRequest { message: "套件至少需要选择一个插件".to_owned() });
        }
        let selected = plugins::table.filter(plugins::tenant_id.is_null().or(plugins::tenant_id.eq(&input.tenant_id)))
            .filter(plugins::id.eq_any(&ids)).order(plugins::id.asc()).for_update()
            .select(PluginSummary::as_select()).load::<PluginSummary>(&mut *conn).await?;
        for (slot, id) in slots {
            if let Some(id) = id {
                if !selected.iter().any(|p| p.id == id && p.slot == slot && p.provider == input.provider) {
                    warn!(tenant_id = ?input.tenant_id, plugin_id = %id, provider = %input.provider, slot, "套件插件不存在或类型不匹配");
                    return Err(AppError::BadRequest { message: format!("{slot} 插件不可用或不符合套件归属、Provider 和插槽: {id}") });
                }
            }
        }
        Ok(diesel::insert_into(plugin_suites::table).values(input)
            .returning(PluginSuiteSummary::as_returning()).get_result(&mut *conn)
            .await.map_err(map_write_error)?)
    }).await?;
    info!(tenant_id = ?suite.tenant_id, plugin_suite_id = %suite.id, provider = %suite.provider,
        request_plugin_id = ?suite.request_plugin_id, buffered_response_plugin_id = ?suite.buffered_response_plugin_id,
        stream_response_plugin_id = ?suite.stream_response_plugin_id, "插件套件已创建，组合固定");
    Ok(suite)
}

pub async fn list(
    conn: &mut AsyncPgConnection,
    tenant_id: Option<String>,
) -> AppResult<Vec<PluginSuiteSummary>> {
    Ok(plugin_suites::table
        .filter(
            plugin_suites::tenant_id
                .is_null()
                .or(plugin_suites::tenant_id.eq(tenant_id)),
        )
        .order((plugin_suites::created_at.desc(), plugin_suites::id.desc()))
        .select(PluginSuiteSummary::as_select())
        .load(conn)
        .await?)
}

pub async fn list_enabled_options(
    conn: &mut AsyncPgConnection,
    tenant_id: Option<String>,
) -> AppResult<Vec<PluginSuiteSummary>> {
    Ok(plugin_suites::table
        .filter(
            plugin_suites::tenant_id
                .is_null()
                .or(plugin_suites::tenant_id.eq(tenant_id)),
        )
        .filter(plugin_suites::enabled.eq(true))
        .order((plugin_suites::provider.asc(), plugin_suites::name.asc()))
        .select(PluginSuiteSummary::as_select())
        .load(conn)
        .await?)
}

pub async fn find_summaries_by_ids(
    conn: &mut AsyncPgConnection,
    tenant_id: String,
    ids: &[Uuid],
) -> AppResult<HashMap<Uuid, PluginSuiteSummary>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    Ok(plugin_suites::table
        .filter(
            plugin_suites::tenant_id
                .is_null()
                .or(plugin_suites::tenant_id.eq(tenant_id)),
        )
        .filter(plugin_suites::id.eq_any(ids))
        .select(PluginSuiteSummary::as_select())
        .load::<PluginSuiteSummary>(conn)
        .await?
        .into_iter()
        .map(|s| (s.id, s))
        .collect())
}

pub async fn find_enabled_suite(
    conn: &mut AsyncPgConnection,
    tenant_id: &str,
    suite_id: Uuid,
    provider: &str,
) -> AppResult<Option<PluginSuiteSummary>> {
    Ok(plugin_suites::table
        .filter(
            plugin_suites::tenant_id
                .is_null()
                .or(plugin_suites::tenant_id.eq(tenant_id)),
        )
        .filter(plugin_suites::id.eq(suite_id))
        .filter(plugin_suites::provider.eq(provider))
        .filter(plugin_suites::enabled.eq(true))
        .select(PluginSuiteSummary::as_select())
        .first(conn)
        .await
        .optional()?)
}

/// Key 写入事务锁定目标套件，删除事务也按套件 UUID 顺序取得行锁。
/// 删除之后不清空 Key 的 suite_id；保留失效引用供鉴权拒绝及 Dashboard 修复。
pub async fn require_enabled_suite_for_provider_write(
    conn: &mut AsyncPgConnection,
    tenant_id: String,
    suite_id: Uuid,
    provider: &str,
) -> AppResult<PluginSuiteSummary> {
    plugin_suites::table
        .filter(
            plugin_suites::tenant_id
                .is_null()
                .or(plugin_suites::tenant_id.eq(tenant_id)),
        )
        .filter(plugin_suites::id.eq(suite_id))
        .filter(plugin_suites::provider.eq(provider))
        .filter(plugin_suites::enabled.eq(true))
        .for_update()
        .select(PluginSuiteSummary::as_select())
        .first(conn)
        .await
        .optional()?
        .ok_or_else(|| AppError::BadRequest {
            message: format!("插件套件不存在、已停用或不属于 {provider}: {suite_id}"),
        })
}

pub async fn load_plugin_metadata(
    conn: &mut AsyncPgConnection,
    tenant_id: Option<&str>,
    ids: &[Uuid],
) -> AppResult<Vec<PluginSummary>> {
    Ok(plugins::table
        .filter(
            plugins::tenant_id
                .is_null()
                .or(plugins::tenant_id.eq(tenant_id)),
        )
        .filter(plugins::id.eq_any(ids))
        .select(PluginSummary::as_select())
        .load(conn)
        .await?)
}

/// 只能在准备请求的同一个数据库快照内调用；冷缓存字节必须在删除可见性变化前取得。
pub async fn load_wasm(
    conn: &mut AsyncPgConnection,
    tenant_id: Option<&str>,
    id: Uuid,
) -> AppResult<Vec<u8>> {
    plugins::table
        .filter(
            plugins::tenant_id
                .is_null()
                .or(plugins::tenant_id.eq(tenant_id)),
        )
        .filter(plugins::id.eq(id))
        .select(plugins::wasm_bytes)
        .first(conn)
        .await
        .optional()?
        .ok_or_else(|| AppError::Plugin {
            message: format!("插件文件不存在: {id}"),
        })
}

pub async fn set_enabled(
    conn: &mut AsyncPgConnection,
    tenant_id: Option<String>,
    suite_id: Uuid,
    enabled: bool,
) -> AppResult<()> {
    let changed = diesel::update(
        plugin_suites::table
            .filter(plugin_suites::tenant_id.is_not_distinct_from(tenant_id.as_deref()))
            .filter(plugin_suites::id.eq(suite_id)),
    )
    .set((
        plugin_suites::enabled.eq(enabled),
        plugin_suites::updated_at.eq(diesel::dsl::now),
    ))
    .execute(conn)
    .await?;
    if changed == 0 {
        return Err(AppError::BadRequest {
            message: format!("插件套件不存在: {suite_id}"),
        });
    }
    info!(tenant_id = ?tenant_id, plugin_suite_id = %suite_id, enabled, "插件套件状态已更新");
    Ok(())
}

/// 删除预览和实际删除共用统计。包含引用公共插件的租户私有套件，以及绑定公共套件的租户。
#[derive(Serialize)]
pub struct DeletionImpact {
    pub suite_count: usize,
    pub affected_tenant_count: usize,
    pub affected_gateway_api_key_count: i64,
}

#[derive(Serialize)]
pub struct DeletePluginResponse {
    pub id: Uuid,
    pub deleted_suite_count: usize,
    pub affected_tenant_count: usize,
    pub affected_gateway_api_key_count: i64,
}

async fn impact_for_suites(
    conn: &mut AsyncPgConnection,
    suites: &[(Uuid, Option<String>)],
) -> AppResult<DeletionImpact> {
    let ids = suites.iter().map(|(id, _)| *id).collect::<Vec<_>>();
    let keys_by_tenant = api_keys::table
        .filter(api_keys::plugin_suite_id.eq_any(&ids))
        .group_by(api_keys::tenant_id)
        .select((api_keys::tenant_id, diesel::dsl::count_star()))
        .load::<(String, i64)>(conn)
        .await?;
    let mut tenants = suites
        .iter()
        .filter_map(|(_, tenant)| tenant.clone())
        .collect::<HashSet<_>>();
    let mut key_count = 0;
    for (tenant, count) in keys_by_tenant {
        tenants.insert(tenant);
        key_count += count;
    }
    Ok(DeletionImpact {
        suite_count: suites.len(),
        affected_tenant_count: tenants.len(),
        affected_gateway_api_key_count: key_count,
    })
}

/// 预览只向资源的管理者开放。只读快照保证三个计数来自同一时点，实际删除时重新统计。
pub async fn deletion_impact(
    conn: &mut AsyncPgConnection,
    tenant_id: Option<String>,
    id: Uuid,
    is_suite: bool,
) -> AppResult<DeletionImpact> {
    conn.build_transaction()
        .repeatable_read()
        .read_only()
        .run::<_, AppError, _>(async |conn| {
            let suites = if is_suite {
                let suite = plugin_suites::table
                    .filter(plugin_suites::tenant_id.is_not_distinct_from(tenant_id.as_deref()))
                    .filter(plugin_suites::id.eq(id))
                    .select((plugin_suites::id, plugin_suites::tenant_id))
                    .first::<(Uuid, Option<String>)>(conn)
                    .await
                    .optional()?
                    .ok_or_else(resource_unavailable)?;
                vec![suite]
            } else {
                plugins::table
                    .filter(plugins::tenant_id.is_not_distinct_from(tenant_id.as_deref()))
                    .filter(plugins::id.eq(id))
                    .select(plugins::id)
                    .first::<Uuid>(conn)
                    .await
                    .optional()?
                    .ok_or_else(resource_unavailable)?;
                // 授权发生在插件本身，依赖扫描必须跨租户，不能复用列表的可见范围。
                plugin_suites::table
                    .filter(
                        plugin_suites::request_plugin_id
                            .eq(id)
                            .or(plugin_suites::buffered_response_plugin_id.eq(id))
                            .or(plugin_suites::stream_response_plugin_id.eq(id)),
                    )
                    .select((plugin_suites::id, plugin_suites::tenant_id))
                    .load(conn)
                    .await?
            };
            impact_for_suites(conn, &suites).await
        })
        .await
}

pub async fn delete_plugin(
    conn: &mut AsyncPgConnection,
    tenant_id: Option<String>,
    id: Uuid,
) -> AppResult<DeletePluginResponse> {
    let deleted = conn
        .transaction::<DeletePluginResponse, AppError, _>(async |conn| {
            plugins::table
                .filter(plugins::tenant_id.is_not_distinct_from(tenant_id.as_deref()))
                .filter(plugins::id.eq(id))
                .for_update()
                .select(plugins::id)
                .first::<Uuid>(&mut *conn)
                .await
                .optional()?
                .ok_or_else(resource_unavailable)?;
            // 与创建组合共用插件行锁，锁定后再跨租户扫描依赖，避免遗漏并发创建的私有套件。
            let suites = plugin_suites::table
                .filter(
                    plugin_suites::request_plugin_id
                        .eq(id)
                        .or(plugin_suites::buffered_response_plugin_id.eq(id))
                        .or(plugin_suites::stream_response_plugin_id.eq(id)),
                )
                .order(plugin_suites::id.asc())
                .for_update()
                .select((plugin_suites::id, plugin_suites::tenant_id))
                .load::<(Uuid, Option<String>)>(&mut *conn)
                .await?;
            let impact = impact_for_suites(&mut *conn, &suites).await?;
            let ids = suites.iter().map(|(id, _)| *id).collect::<Vec<_>>();
            let deleted_suite_count =
                diesel::delete(plugin_suites::table.filter(plugin_suites::id.eq_any(ids)))
                    .execute(&mut *conn)
                    .await?;
            diesel::delete(
                plugins::table
                    .filter(plugins::tenant_id.is_not_distinct_from(tenant_id.as_deref()))
                    .filter(plugins::id.eq(id)),
            )
            .execute(&mut *conn)
            .await?;
            Ok(DeletePluginResponse {
                id,
                deleted_suite_count,
                affected_tenant_count: impact.affected_tenant_count,
                affected_gateway_api_key_count: impact.affected_gateway_api_key_count,
            })
        })
        .await?;
    info!(tenant_id = ?tenant_id, plugin_id = %id, deleted_suite_count = deleted.deleted_suite_count,
        affected_tenant_count = deleted.affected_tenant_count, affected_gateway_api_key_count = deleted.affected_gateway_api_key_count,
        "插件及全部引用套件已删除，跨租户 Key 保留失效引用，编译缓存保留");
    Ok(deleted)
}

pub async fn delete_suite(
    conn: &mut AsyncPgConnection,
    tenant_id: Option<String>,
    id: Uuid,
) -> AppResult<DeletePluginResponse> {
    let deleted = conn
        .transaction::<DeletePluginResponse, AppError, _>(async |conn| {
            let suite = plugin_suites::table
                .filter(plugin_suites::tenant_id.is_not_distinct_from(tenant_id.as_deref()))
                .filter(plugin_suites::id.eq(id))
                .for_update()
                .select((plugin_suites::id, plugin_suites::tenant_id))
                .first::<(Uuid, Option<String>)>(&mut *conn)
                .await
                .optional()?
                .ok_or_else(resource_unavailable)?;
            let impact = impact_for_suites(&mut *conn, &[suite]).await?;
            let deleted_suite_count = diesel::delete(
                plugin_suites::table
                    .filter(plugin_suites::tenant_id.is_not_distinct_from(tenant_id.as_deref()))
                    .filter(plugin_suites::id.eq(id)),
            )
            .execute(&mut *conn)
            .await?;
            Ok(DeletePluginResponse {
                id,
                deleted_suite_count,
                affected_tenant_count: impact.affected_tenant_count,
                affected_gateway_api_key_count: impact.affected_gateway_api_key_count,
            })
        })
        .await?;
    info!(tenant_id = ?tenant_id, plugin_suite_id = %id, affected_tenant_count = deleted.affected_tenant_count,
        affected_gateway_api_key_count = deleted.affected_gateway_api_key_count,
        "套件已删除，独立插件及所有租户 Key 的套件引用保留");
    Ok(deleted)
}

fn resource_unavailable() -> AppError {
    AppError::BadRequest {
        message: "插件或套件不存在，或不属于当前管理范围".to_owned(),
    }
}

fn map_write_error(source: diesel::result::Error) -> AppError {
    if matches!(
        source,
        diesel::result::Error::DatabaseError(diesel::result::DatabaseErrorKind::UniqueViolation, _)
    ) {
        return AppError::BadRequest {
            message: "当前归属、Provider 下套件名称或同一插槽下插件名称已存在".to_owned(),
        };
    }
    source.into()
}
