use std::collections::HashMap;

use diesel::prelude::*;
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl};
use tracing::{info, warn};
use uuid::Uuid;

use crate::{
    err::{AppError, AppResult},
    model::schema::api_keys,
};

use super::{
    ABI_VERSION,
    model::{
        NewPluginSuite, NewPluginSuiteArtifact, NewPluginSuiteRelease, PluginArtifact,
        PluginArtifactBinding, PluginArtifactSummary, PluginArtifactUpload, PluginBinding,
        PluginReleaseSummary, PluginSlot, PluginSuite,
        schema::{plugin_suite_artifacts, plugin_suite_releases, plugin_suites},
    },
};

type ReleaseRow = (
    Uuid,
    Uuid,
    Uuid,
    String,
    String,
    String,
    bool,
    i64,
    String,
    chrono::DateTime<chrono::Utc>,
);

type ArtifactSummaryRow = (Uuid, Uuid, String, i32, String, i64);

pub struct DeletedPluginSuite {
    pub suite: PluginSuite,
    pub release_count: usize,
    pub artifact_ids: Vec<Uuid>,
    pub unbound_gateway_api_key_count: usize,
}

macro_rules! release_select {
    () => {
        (
            plugin_suite_releases::id,
            plugin_suite_releases::suite_id,
            plugin_suites::tenant_id,
            plugin_suites::name,
            plugin_suites::description,
            plugin_suites::provider,
            plugin_suites::enabled,
            plugin_suite_releases::version,
            plugin_suite_releases::manifest_sha256,
            plugin_suite_releases::published_at,
        )
    };
}

macro_rules! suite_select {
    () => {
        (
            plugin_suites::id,
            plugin_suites::tenant_id,
            plugin_suites::name,
            plugin_suites::description,
            plugin_suites::provider,
            plugin_suites::enabled,
        )
    };
}

pub async fn create_and_publish(
    conn: &mut AsyncPgConnection,
    tenant_id: Uuid,
    created_by: Uuid,
    name: String,
    description: String,
    provider: String,
    manifest_sha256: String,
    artifacts: Vec<PluginArtifactUpload>,
) -> AppResult<PluginReleaseSummary> {
    ensure_artifact_set(&artifacts)?;
    let row = conn
        .transaction::<ReleaseRow, AppError, _>(async |conn| {
            let suite = diesel::insert_into(plugin_suites::table)
                .values(NewPluginSuite {
                    tenant_id,
                    name,
                    description,
                    provider,
                    created_by,
                })
                .returning(suite_select!())
                .get_result::<PluginSuite>(&mut *conn)
                .await
                .map_err(map_suite_write_error)?;
            insert_release(
                &mut *conn,
                &suite,
                created_by,
                1,
                manifest_sha256,
                artifacts,
            )
            .await
        })
        .await?;
    let summary = load_summary_from_row(conn, row).await?;
    log_published(&summary, "WASM 插件套件首个版本已持久化并发布");
    Ok(summary)
}

pub async fn publish_release(
    conn: &mut AsyncPgConnection,
    tenant_id: Uuid,
    suite_id: Uuid,
    created_by: Uuid,
    manifest_sha256: String,
    artifacts: Vec<PluginArtifactUpload>,
) -> AppResult<PluginReleaseSummary> {
    ensure_artifact_set(&artifacts)?;
    let row = conn
        .transaction::<ReleaseRow, AppError, _>(async |conn| {
            let suite = plugin_suites::table
                .filter(plugin_suites::id.eq(suite_id))
                .filter(plugin_suites::tenant_id.eq(tenant_id))
                .for_update()
                .select(suite_select!())
                .first::<PluginSuite>(&mut *conn)
                .await
                .optional()?
                .ok_or_else(|| AppError::BadRequest {
                    message: format!("插件套件不存在: {suite_id}"),
                })?;
            let current_version = plugin_suite_releases::table
                .filter(plugin_suite_releases::suite_id.eq(suite_id))
                .select(diesel::dsl::max(plugin_suite_releases::version))
                .first::<Option<i64>>(&mut *conn)
                .await?
                .unwrap_or(0);
            insert_release(
                &mut *conn,
                &suite,
                created_by,
                current_version.saturating_add(1),
                manifest_sha256,
                artifacts,
            )
            .await
        })
        .await?;
    let summary = load_summary_from_row(conn, row).await?;
    log_published(&summary, "WASM 插件套件新版本已发布");
    Ok(summary)
}

async fn insert_release(
    conn: &mut AsyncPgConnection,
    suite: &PluginSuite,
    created_by: Uuid,
    version: i64,
    manifest_sha256: String,
    artifacts: Vec<PluginArtifactUpload>,
) -> AppResult<ReleaseRow> {
    let release = diesel::insert_into(plugin_suite_releases::table)
        .values(NewPluginSuiteRelease {
            suite_id: suite.id,
            version,
            manifest_sha256,
            created_by,
        })
        .returning((
            plugin_suite_releases::id,
            plugin_suite_releases::suite_id,
            plugin_suite_releases::version,
            plugin_suite_releases::manifest_sha256,
            plugin_suite_releases::published_at,
        ))
        .get_result::<(Uuid, Uuid, i64, String, chrono::DateTime<chrono::Utc>)>(conn)
        .await
        .map_err(map_release_write_error)?;

    let artifact_rows = artifacts
        .into_iter()
        .map(|artifact| {
            let wasm_size =
                i64::try_from(artifact.wasm_bytes.len()).map_err(|_| AppError::BadRequest {
                    message: format!("{} 插槽 WASM 文件大小超出支持范围", artifact.slot.as_str()),
                })?;
            Ok(NewPluginSuiteArtifact {
                release_id: release.0,
                slot: artifact.slot.as_str().to_owned(),
                abi_version: ABI_VERSION,
                wasm_sha256: artifact.wasm_sha256,
                wasm_size,
                wasm_bytes: artifact.wasm_bytes,
            })
        })
        .collect::<AppResult<Vec<_>>>()?;
    diesel::insert_into(plugin_suite_artifacts::table)
        .values(&artifact_rows)
        .execute(conn)
        .await
        .map_err(map_release_write_error)?;

    Ok((
        release.0,
        release.1,
        suite.tenant_id,
        suite.name.clone(),
        suite.description.clone(),
        suite.provider.clone(),
        suite.enabled,
        release.2,
        release.3,
        release.4,
    ))
}

pub async fn list(
    conn: &mut AsyncPgConnection,
    tenant_id: Uuid,
) -> AppResult<Vec<PluginReleaseSummary>> {
    let rows = plugin_suite_releases::table
        .inner_join(plugin_suites::table.on(plugin_suites::id.eq(plugin_suite_releases::suite_id)))
        .filter(plugin_suites::tenant_id.eq(tenant_id))
        .order((
            plugin_suites::created_at.desc(),
            plugin_suite_releases::version.desc(),
        ))
        .select(release_select!())
        .load::<ReleaseRow>(conn)
        .await?;
    attach_artifacts(conn, rows).await
}

pub async fn list_enabled_options(
    conn: &mut AsyncPgConnection,
    tenant_id: Uuid,
) -> AppResult<Vec<PluginReleaseSummary>> {
    let rows = plugin_suite_releases::table
        .inner_join(plugin_suites::table.on(plugin_suites::id.eq(plugin_suite_releases::suite_id)))
        .filter(plugin_suites::enabled.eq(true))
        .filter(plugin_suites::tenant_id.eq(tenant_id))
        .order((
            plugin_suites::provider.asc(),
            plugin_suites::name.asc(),
            plugin_suite_releases::version.desc(),
        ))
        .select(release_select!())
        .load::<ReleaseRow>(conn)
        .await?;
    attach_artifacts(conn, rows).await
}

pub async fn find_suite(
    conn: &mut AsyncPgConnection,
    tenant_id: Uuid,
    suite_id: Uuid,
) -> AppResult<Option<PluginSuite>> {
    plugin_suites::table
        .filter(plugin_suites::id.eq(suite_id))
        .filter(plugin_suites::tenant_id.eq(tenant_id))
        .select(suite_select!())
        .first::<PluginSuite>(conn)
        .await
        .optional()
        .map_err(Into::into)
}

pub async fn find_summaries_by_ids(
    conn: &mut AsyncPgConnection,
    tenant_id: Uuid,
    ids: &[Uuid],
) -> AppResult<HashMap<Uuid, PluginReleaseSummary>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = plugin_suite_releases::table
        .inner_join(plugin_suites::table.on(plugin_suites::id.eq(plugin_suite_releases::suite_id)))
        .filter(plugin_suite_releases::id.eq_any(ids))
        .filter(plugin_suites::tenant_id.eq(tenant_id))
        .select(release_select!())
        .load::<ReleaseRow>(conn)
        .await?;
    Ok(attach_artifacts(conn, rows)
        .await?
        .into_iter()
        .map(|summary| (summary.id, summary))
        .collect())
}

pub async fn require_enabled_release_for_provider(
    conn: &mut AsyncPgConnection,
    tenant_id: Uuid,
    release_id: Uuid,
    provider: &str,
) -> AppResult<PluginReleaseSummary> {
    let row = plugin_suite_releases::table
        .inner_join(plugin_suites::table.on(plugin_suites::id.eq(plugin_suite_releases::suite_id)))
        .filter(plugin_suite_releases::id.eq(release_id))
        .filter(plugin_suites::enabled.eq(true))
        .filter(plugin_suites::tenant_id.eq(tenant_id))
        .filter(plugin_suites::provider.eq(provider))
        .select(release_select!())
        .first::<ReleaseRow>(conn)
        .await
        .optional()?;
    let row = row.ok_or_else(|| AppError::BadRequest {
        message: format!("插件套件发布版本不存在、已停用或不属于 {provider}: {release_id}"),
    })?;
    load_summary_from_row(conn, row).await
}

/// 为网关 Key绑定写入锁定 release 所属套件，并返回仍启用且 Provider 匹配的版本。
///
/// 普通请求鉴权只读插件元数据，不能承担长事务行锁；绑定写入单独使用本入口，统一与
/// 套件删除保持“套件 -> 网关 Key”的锁序。删除先取得套件锁时，本次绑定会在等待后发现
/// 套件已经不存在；绑定先取得套件锁时，删除会在其提交后解除刚写入的绑定。
pub async fn require_enabled_release_for_provider_write(
    conn: &mut AsyncPgConnection,
    tenant_id: Uuid,
    release_id: Uuid,
    provider: &str,
) -> AppResult<PluginReleaseSummary> {
    let suite_id = plugin_suite_releases::table
        .filter(plugin_suite_releases::id.eq(release_id))
        .select(plugin_suite_releases::suite_id)
        .first::<Uuid>(&mut *conn)
        .await
        .optional()?
        .ok_or_else(|| AppError::BadRequest {
            message: format!("插件套件发布版本不存在、已停用或不属于 {provider}: {release_id}"),
        })?;
    let suite_exists = plugin_suites::table
        .filter(plugin_suites::id.eq(suite_id))
        .filter(plugin_suites::tenant_id.eq(tenant_id))
        .filter(plugin_suites::enabled.eq(true))
        .filter(plugin_suites::provider.eq(provider))
        .for_update()
        .select(plugin_suites::id)
        .first::<Uuid>(&mut *conn)
        .await
        .optional()?
        .is_some();
    if !suite_exists {
        return Err(AppError::BadRequest {
            message: format!("插件套件发布版本不存在、已停用或不属于 {provider}: {release_id}"),
        });
    }
    require_enabled_release_for_provider(&mut *conn, tenant_id, release_id, provider).await
}

pub async fn load_binding(
    conn: &mut AsyncPgConnection,
    tenant_id: Uuid,
    release_id: Uuid,
    provider: &str,
) -> AppResult<PluginBinding> {
    let summary =
        require_enabled_release_for_provider(conn, tenant_id, release_id, provider).await?;
    binding_from_summary(summary)
}

pub async fn load_artifact(
    conn: &mut AsyncPgConnection,
    suite_binding: &PluginBinding,
    artifact_id: Uuid,
) -> AppResult<PluginArtifact> {
    let artifact_binding = suite_binding
        .artifacts
        .iter()
        .find(|artifact| artifact.id == artifact_id)
        .cloned()
        .ok_or_else(|| AppError::Plugin {
            message: format!("artifact 不属于鉴权套件快照: {artifact_id}"),
        })?;
    let row = plugin_suite_artifacts::table
        .inner_join(
            plugin_suite_releases::table
                .on(plugin_suite_releases::id.eq(plugin_suite_artifacts::release_id)),
        )
        .inner_join(plugin_suites::table.on(plugin_suites::id.eq(plugin_suite_releases::suite_id)))
        .filter(plugin_suite_artifacts::id.eq(artifact_id))
        .filter(plugin_suite_artifacts::release_id.eq(suite_binding.release_id))
        .filter(plugin_suites::enabled.eq(true))
        .filter(plugin_suites::tenant_id.eq(suite_binding.tenant_id))
        .select((
            plugin_suite_artifacts::wasm_sha256,
            plugin_suite_artifacts::wasm_bytes,
        ))
        .first::<(String, Vec<u8>)>(conn)
        .await
        .optional()?
        .ok_or_else(|| AppError::Plugin {
            message: format!("插件 artifact 不存在、套件已停用或绑定已失效: {artifact_id}"),
        })?;
    if row.0 != artifact_binding.wasm_sha256 {
        return Err(AppError::Plugin {
            message: format!("插件 artifact 摘要与鉴权快照不一致: {artifact_id}"),
        });
    }
    Ok(PluginArtifact {
        binding: artifact_binding,
        suite_binding: suite_binding.clone(),
        wasm_bytes: row.1,
    })
}

pub async fn set_enabled(
    conn: &mut AsyncPgConnection,
    tenant_id: Uuid,
    suite_id: Uuid,
    enabled: bool,
) -> AppResult<()> {
    let changed = diesel::update(
        plugin_suites::table
            .filter(plugin_suites::id.eq(suite_id))
            .filter(plugin_suites::tenant_id.eq(tenant_id)),
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
    info!(plugin_suite_id = %suite_id, enabled, "WASM 插件套件启停状态已更新");
    Ok(())
}

/// 永久删除插件套件的全部 release 和 artifact，并解除所有网关 Key绑定。
///
/// 项目不使用数据库外键，因此必须显式按依赖顺序清理。网关 Key 本身及其模型白名单、
/// 分组归属和启用状态全部保留，只把 `plugin_release_id` 设为空并推进更新时间；后续请求
/// 自动回落到 Provider 原生流程。
pub async fn delete_suite(
    conn: &mut AsyncPgConnection,
    tenant_id: Uuid,
    suite_id: Uuid,
) -> AppResult<DeletedPluginSuite> {
    let deleted = conn
        .transaction::<DeletedPluginSuite, AppError, _>(async |conn| {
            let suite = plugin_suites::table
                .filter(plugin_suites::id.eq(suite_id))
                .filter(plugin_suites::tenant_id.eq(tenant_id))
                .for_update()
                .select(suite_select!())
                .first::<PluginSuite>(&mut *conn)
                .await
                .optional()?
                .ok_or_else(|| AppError::BadRequest {
                    message: format!("插件套件不存在: {suite_id}"),
                })?;
            let release_ids = plugin_suite_releases::table
                .filter(plugin_suite_releases::suite_id.eq(suite.id))
                .order(plugin_suite_releases::version.asc())
                .select(plugin_suite_releases::id)
                .load::<Uuid>(&mut *conn)
                .await?;
            let artifact_ids = if release_ids.is_empty() {
                Vec::new()
            } else {
                plugin_suite_artifacts::table
                    .filter(plugin_suite_artifacts::release_id.eq_any(&release_ids))
                    .select(plugin_suite_artifacts::id)
                    .load::<Uuid>(&mut *conn)
                    .await?
            };

            // 分组删除也会批量锁定网关 Key。两条删除链路都先按稳定 UUID 顺序取得全部
            // 行锁，再执行 DELETE/UPDATE，避免不同索引扫描顺序造成交叉持锁和死锁。
            let bound_gateway_api_key_ids = if release_ids.is_empty() {
                Vec::new()
            } else {
                api_keys::table
                    .filter(api_keys::plugin_release_id.eq_any(&release_ids))
                    .order(api_keys::id.asc())
                    .for_update()
                    .select(api_keys::id)
                    .load::<Uuid>(&mut *conn)
                    .await?
            };
            let unbound_gateway_api_key_count = if bound_gateway_api_key_ids.is_empty() {
                0
            } else {
                diesel::update(
                    api_keys::table.filter(api_keys::id.eq_any(&bound_gateway_api_key_ids)),
                )
                .set((
                    api_keys::plugin_release_id.eq(None::<Uuid>),
                    api_keys::updated_at.eq(diesel::dsl::now),
                ))
                .execute(&mut *conn)
                .await?
            };
            if unbound_gateway_api_key_count != bound_gateway_api_key_ids.len() {
                return Err(AppError::DbQuery {
                    message: format!(
                        "删除插件套件时网关 Key 数量发生并发变化: suite_id={}, expected={}, actual={unbound_gateway_api_key_count}",
                        suite.id,
                        bound_gateway_api_key_ids.len(),
                    ),
                });
            }
            let deleted_artifact_count = if release_ids.is_empty() {
                0
            } else {
                diesel::delete(
                    plugin_suite_artifacts::table
                        .filter(plugin_suite_artifacts::release_id.eq_any(&release_ids)),
                )
                .execute(&mut *conn)
                .await?
            };
            if deleted_artifact_count != artifact_ids.len() {
                return Err(AppError::DbQuery {
                    message: format!(
                        "删除插件套件时 artifact 数量发生并发变化: suite_id={}, expected={}, actual={deleted_artifact_count}",
                        suite.id,
                        artifact_ids.len(),
                    ),
                });
            }
            let deleted_release_count = diesel::delete(
                plugin_suite_releases::table
                    .filter(plugin_suite_releases::suite_id.eq(suite.id)),
            )
            .execute(&mut *conn)
            .await?;
            if deleted_release_count != release_ids.len() {
                return Err(AppError::DbQuery {
                    message: format!(
                        "删除插件套件时 release 数量发生并发变化: suite_id={}, expected={}, actual={deleted_release_count}",
                        suite.id,
                        release_ids.len(),
                    ),
                });
            }
            let deleted_suite_count =
                diesel::delete(plugin_suites::table.filter(plugin_suites::id.eq(suite.id)))
                    .execute(&mut *conn)
                    .await?;
            if deleted_suite_count != 1 {
                return Err(AppError::DbQuery {
                    message: format!("删除插件套件主记录失败: {}", suite.id),
                });
            }

            Ok(DeletedPluginSuite {
                suite,
                release_count: release_ids.len(),
                artifact_ids,
                unbound_gateway_api_key_count,
            })
        })
        .await?;

    info!(
        plugin_suite_id = %deleted.suite.id,
        plugin_suite_name = %deleted.suite.name,
        provider = %deleted.suite.provider,
        release_count = deleted.release_count,
        artifact_count = deleted.artifact_ids.len(),
        unbound_gateway_api_key_count = deleted.unbound_gateway_api_key_count,
        "WASM 插件套件已永久删除，关联网关 Key已解除插件绑定"
    );
    Ok(deleted)
}

fn ensure_artifact_set(artifacts: &[PluginArtifactUpload]) -> AppResult<()> {
    if artifacts.is_empty() {
        return Err(AppError::BadRequest {
            message: "插件套件至少需要上传一个 WASM Component".to_owned(),
        });
    }
    for slot in PluginSlot::ALL {
        if artifacts
            .iter()
            .filter(|artifact| artifact.slot == slot)
            .count()
            > 1
        {
            return Err(AppError::BadRequest {
                message: format!("插件套件 {} 插槽重复", slot.as_str()),
            });
        }
    }
    Ok(())
}

async fn attach_artifacts(
    conn: &mut AsyncPgConnection,
    rows: Vec<ReleaseRow>,
) -> AppResult<Vec<PluginReleaseSummary>> {
    let release_ids = rows.iter().map(|row| row.0).collect::<Vec<_>>();
    let artifact_rows = if release_ids.is_empty() {
        Vec::new()
    } else {
        plugin_suite_artifacts::table
            .filter(plugin_suite_artifacts::release_id.eq_any(&release_ids))
            .order((
                plugin_suite_artifacts::release_id.asc(),
                plugin_suite_artifacts::slot.asc(),
            ))
            .select((
                plugin_suite_artifacts::id,
                plugin_suite_artifacts::release_id,
                plugin_suite_artifacts::slot,
                plugin_suite_artifacts::abi_version,
                plugin_suite_artifacts::wasm_sha256,
                plugin_suite_artifacts::wasm_size,
            ))
            .load::<ArtifactSummaryRow>(conn)
            .await?
    };
    let mut artifacts_by_release = HashMap::<Uuid, Vec<PluginArtifactSummary>>::new();
    for row in artifact_rows {
        let slot = PluginSlot::parse(&row.2).ok_or_else(|| AppError::DbQuery {
            message: format!("插件 artifact slot 非法: {}", row.2),
        })?;
        if row.3 != ABI_VERSION {
            return Err(AppError::DbQuery {
                message: format!("插件 artifact ABI 不受支持: id={}, abi={}", row.0, row.3),
            });
        }
        artifacts_by_release
            .entry(row.1)
            .or_default()
            .push(PluginArtifactSummary {
                id: row.0,
                slot,
                abi_version: row.3,
                wasm_sha256: row.4,
                wasm_size: usize::try_from(row.5).map_err(|_| AppError::DbQuery {
                    message: format!("插件 artifact 大小非法: {}", row.5),
                })?,
            });
    }
    rows.into_iter()
        .map(|row| {
            let artifacts = artifacts_by_release.remove(&row.0).unwrap_or_default();
            if artifacts.is_empty() {
                return Err(AppError::DbQuery {
                    message: format!("插件套件 release 缺少 artifact: {}", row.0),
                });
            }
            Ok(summary_from_parts(row, artifacts))
        })
        .collect()
}

async fn load_summary_from_row(
    conn: &mut AsyncPgConnection,
    row: ReleaseRow,
) -> AppResult<PluginReleaseSummary> {
    attach_artifacts(conn, vec![row])
        .await?
        .pop()
        .ok_or_else(|| AppError::DbQuery {
            message: "插件套件发布查询没有返回结果".to_owned(),
        })
}

fn summary_from_parts(
    row: ReleaseRow,
    artifacts: Vec<PluginArtifactSummary>,
) -> PluginReleaseSummary {
    PluginReleaseSummary {
        id: row.0,
        suite_id: row.1,
        tenant_id: row.2,
        suite_name: row.3,
        description: row.4,
        provider: row.5,
        suite_enabled: row.6,
        version: row.7,
        manifest_sha256: row.8,
        artifacts,
        published_at: row.9,
    }
}

fn binding_from_summary(summary: PluginReleaseSummary) -> AppResult<PluginBinding> {
    let artifacts = summary
        .artifacts
        .into_iter()
        .map(|artifact| PluginArtifactBinding {
            id: artifact.id,
            slot: artifact.slot,
            abi_version: artifact.abi_version,
            wasm_sha256: artifact.wasm_sha256,
        })
        .collect::<Vec<_>>();
    if artifacts.is_empty() {
        return Err(AppError::Plugin {
            message: format!("插件套件 release 缺少 artifact: {}", summary.id),
        });
    }
    Ok(PluginBinding {
        release_id: summary.id,
        suite_id: summary.suite_id,
        tenant_id: summary.tenant_id,
        suite_name: summary.suite_name,
        provider: summary.provider,
        version: summary.version,
        manifest_sha256: summary.manifest_sha256,
        artifacts,
    })
}

fn log_published(summary: &PluginReleaseSummary, message: &'static str) {
    info!(
        plugin_suite_id = %summary.suite_id,
        plugin_release_id = %summary.id,
        plugin_suite_name = %summary.suite_name,
        provider = %summary.provider,
        version = summary.version,
        manifest_sha256 = %summary.manifest_sha256,
        artifact_count = summary.artifacts.len(),
        "{message}"
    );
}

fn map_suite_write_error(source: diesel::result::Error) -> AppError {
    if matches!(
        source,
        diesel::result::Error::DatabaseError(diesel::result::DatabaseErrorKind::UniqueViolation, _)
    ) {
        warn!("同一 Provider 下插件套件名称重复，拒绝创建");
        return AppError::BadRequest {
            message: "同一 Provider 下插件套件名称已存在".to_owned(),
        };
    }
    AppError::DbQuery {
        message: source.to_string(),
    }
}

fn map_release_write_error(source: diesel::result::Error) -> AppError {
    if matches!(
        source,
        diesel::result::Error::DatabaseError(diesel::result::DatabaseErrorKind::UniqueViolation, _)
    ) {
        warn!("插件套件版本、manifest 或 artifact 插槽重复，拒绝发布");
        return AppError::BadRequest {
            message: "相同插件套件内容已经发布，不能重复创建版本".to_owned(),
        };
    }
    AppError::DbQuery {
        message: source.to_string(),
    }
}
