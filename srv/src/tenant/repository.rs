use chrono::Utc;
use diesel::prelude::*;
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl};
use tracing::{info, warn};
use uuid::Uuid;

use super::model::{Tenant, TenantSummary, schema};
use crate::{
    err::{AppError, AppResult},
    user,
};

const MAX_TENANT_NAME_BYTES: usize = 512;
const MAX_TENANT_NAME_CHARS: usize = 128;
const MAX_TENANT_CODE_BYTES: usize = 128;
const GENERATED_CODE_SUFFIX_LENGTH: usize = 6;
const MAX_GENERATED_CODE_NAME_BYTES: usize =
    MAX_TENANT_CODE_BYTES - GENERATED_CODE_SUFFIX_LENGTH - 1;

#[derive(Insertable)]
#[diesel(table_name = schema::tenants)]
struct NewTenant {
    name: String,
    created_by: Uuid,
}

#[derive(Insertable)]
#[diesel(table_name = schema::tenant_codes)]
struct NewTenantCode {
    code: String,
    tenant_id: Uuid,
    created_by: Uuid,
}

pub fn normalize_name(name: String) -> AppResult<String> {
    let name = name.trim().to_owned();
    let char_count = name.chars().count();
    if char_count == 0 || char_count > MAX_TENANT_NAME_CHARS || name.len() > MAX_TENANT_NAME_BYTES {
        return Err(AppError::BadRequest {
            message: format!(
                "租户名称必须为 1..={MAX_TENANT_NAME_CHARS} 个字符且不超过 {MAX_TENANT_NAME_BYTES} 字节"
            ),
        });
    }
    Ok(name)
}

pub fn normalize_code(code: String) -> AppResult<String> {
    let code = code.trim().to_owned();
    if code.is_empty() || code.len() > MAX_TENANT_CODE_BYTES {
        return Err(AppError::BadRequest {
            message: format!("租户码必须为 1..={MAX_TENANT_CODE_BYTES} 字节"),
        });
    }
    if code.chars().any(char::is_whitespace) {
        return Err(AppError::BadRequest {
            message: "租户码不能包含空白字符".to_owned(),
        });
    }
    Ok(code)
}

pub async fn create(
    conn: &mut AsyncPgConnection,
    name: String,
    password: Option<String>,
    actor_id: Uuid,
) -> AppResult<TenantSummary> {
    let name = normalize_name(name)?;
    let code = generate_code(&name)?;
    let prepared_owner = match password.filter(|password| !password.is_empty()) {
        Some(password) => Some(user::prepare_tenant_owner(&name, password).await?),
        None => None,
    };
    let (summary, owner) = conn
        .transaction::<(TenantSummary, Option<user::User>), AppError, _>(async |conn| {
            let tenant = diesel::insert_into(schema::tenants::table)
                .values(NewTenant {
                    name,
                    created_by: actor_id,
                })
                .returning(Tenant::as_returning())
                .get_result::<Tenant>(&mut *conn)
                .await
                .map_err(map_create_error)?;
            diesel::insert_into(schema::tenant_codes::table)
                .values(NewTenantCode {
                    code: code.clone(),
                    tenant_id: tenant.id,
                    created_by: actor_id,
                })
                .execute(&mut *conn)
                .await
                .map_err(map_create_error)?;
            let owner = match prepared_owner {
                Some(owner) => {
                    let tenant_id = tenant.id;
                    Some(user::create_prepared_tenant_owner(&mut *conn, tenant_id, owner).await?)
                }
                None => None,
            };
            Ok((
                TenantSummary {
                    tenant,
                    code: Some(code),
                },
                owner,
            ))
        })
        .await?;
    info!(
        platform_admin_id = %actor_id,
        tenant_id = %summary.tenant.id,
        tenant_name = %summary.tenant.name,
        owner_created = owner.is_some(),
        owner_id = ?owner.as_ref().map(|owner| owner.id),
        owner_username = ?owner.as_ref().map(|owner| owner.username.as_str()),
        "平台管理员已创建租户并分发租户码"
    );
    Ok(summary)
}

fn generate_code(name: &str) -> AppResult<String> {
    use rand::distr::{Alphanumeric, SampleString};

    if name.len() > MAX_GENERATED_CODE_NAME_BYTES {
        return Err(AppError::BadRequest {
            message: format!(
                "自动生成租户码时，租户名称的 UTF-8 编码不能超过 {MAX_GENERATED_CODE_NAME_BYTES} 字节"
            ),
        });
    }
    let suffix = Alphanumeric.sample_string(&mut rand::rng(), GENERATED_CODE_SUFFIX_LENGTH);
    Ok(format!("{name}-{suffix}"))
}

pub async fn list(conn: &mut AsyncPgConnection) -> AppResult<Vec<TenantSummary>> {
    let rows = schema::tenants::table
        .left_join(
            schema::tenant_codes::table.on(schema::tenant_codes::tenant_id.eq(schema::tenants::id)),
        )
        .order((
            schema::tenants::created_at.desc(),
            schema::tenants::id.desc(),
        ))
        .select((Tenant::as_select(), schema::tenant_codes::code.nullable()))
        .load::<(Tenant, Option<String>)>(conn)
        .await
        .map_err(db_error)?;
    Ok(rows
        .into_iter()
        .map(|(tenant, code)| TenantSummary { tenant, code })
        .collect())
}

/// 用量统计只读取租户稳定标识和展示名称，避免平台概览读取租户码等无关字段。
pub async fn list_usage_names(conn: &mut AsyncPgConnection) -> AppResult<Vec<(Uuid, String)>> {
    schema::tenants::table
        .select((schema::tenants::id, schema::tenants::name))
        .load::<(Uuid, String)>(conn)
        .await
        .map_err(db_error)
}

pub async fn find_enabled_by_code_for_update(
    conn: &mut AsyncPgConnection,
    code: &str,
) -> AppResult<Option<Tenant>> {
    let Some(tenant_id) = schema::tenant_codes::table
        .filter(schema::tenant_codes::code.eq(code))
        .select(schema::tenant_codes::tenant_id)
        .first::<Uuid>(&mut *conn)
        .await
        .optional()
        .map_err(db_error)?
    else {
        return Ok(None);
    };

    // tenant 行是注册、换码、撤码和停用的统一并发锁。拿锁后重新确认 code 映射，避免
    // 注册与平台操作交错时使用已经被撤销或替换的旧码，也避免多表 FOR UPDATE 锁序不明。
    let tenant = schema::tenants::table
        .filter(schema::tenants::id.eq(tenant_id))
        .for_update()
        .select(Tenant::as_select())
        .first::<Tenant>(&mut *conn)
        .await
        .optional()
        .map_err(db_error)?;
    let Some(tenant) = tenant.filter(|tenant| tenant.enabled) else {
        return Ok(None);
    };
    let code_still_valid = schema::tenant_codes::table
        .filter(schema::tenant_codes::code.eq(code))
        .filter(schema::tenant_codes::tenant_id.eq(tenant.id))
        .select(schema::tenant_codes::code)
        .first::<String>(&mut *conn)
        .await
        .optional()
        .map_err(db_error)?
        .is_some();
    Ok(code_still_valid.then_some(tenant))
}

pub async fn find_enabled_by_code(
    conn: &mut AsyncPgConnection,
    code: &str,
) -> AppResult<Option<Tenant>> {
    schema::tenant_codes::table
        .inner_join(
            schema::tenants::table.on(schema::tenants::id.eq(schema::tenant_codes::tenant_id)),
        )
        .filter(schema::tenant_codes::code.eq(code))
        .filter(schema::tenants::enabled.eq(true))
        .select(Tenant::as_select())
        .first::<Tenant>(conn)
        .await
        .optional()
        .map_err(db_error)
}

pub async fn require_enabled(conn: &mut AsyncPgConnection, id: Uuid) -> AppResult<Tenant> {
    schema::tenants::table
        .filter(schema::tenants::id.eq(id))
        .filter(schema::tenants::enabled.eq(true))
        .select(Tenant::as_select())
        .first::<Tenant>(conn)
        .await
        .map_err(|source| match source {
            diesel::result::Error::NotFound => AppError::Forbidden,
            source => db_error(source),
        })
}

pub async fn set_enabled(
    conn: &mut AsyncPgConnection,
    id: Uuid,
    enabled: bool,
    actor_id: Uuid,
) -> AppResult<Tenant> {
    let now = Utc::now();
    let tenant = diesel::update(schema::tenants::table.filter(schema::tenants::id.eq(id)))
        .set((
            schema::tenants::enabled.eq(enabled),
            schema::tenants::disabled_at.eq((!enabled).then_some(now)),
            schema::tenants::updated_at.eq(now),
        ))
        .returning(Tenant::as_returning())
        .get_result::<Tenant>(conn)
        .await
        .map_err(|source| match source {
            diesel::result::Error::NotFound => AppError::BadRequest {
                message: format!("租户不存在: {id}"),
            },
            source => db_error(source),
        })?;
    info!(platform_admin_id = %actor_id, tenant_id = %id, enabled, "平台管理员已更新租户状态");
    Ok(tenant)
}

pub async fn regenerate_code(
    conn: &mut AsyncPgConnection,
    tenant_id: Uuid,
    actor_id: Uuid,
) -> AppResult<TenantSummary> {
    let summary = conn
        .transaction::<TenantSummary, AppError, _>(async |conn| {
            let tenant = schema::tenants::table
                .filter(schema::tenants::id.eq(tenant_id))
                .for_update()
                .select(Tenant::as_select())
                .first::<Tenant>(&mut *conn)
                .await
                .map_err(|source| match source {
                    diesel::result::Error::NotFound => AppError::BadRequest {
                        message: format!("租户不存在: {tenant_id}"),
                    },
                    source => db_error(source),
                })?;
            let code = generate_code(&tenant.name)?;
            diesel::delete(
                schema::tenant_codes::table.filter(schema::tenant_codes::tenant_id.eq(tenant_id)),
            )
            .execute(&mut *conn)
            .await
            .map_err(db_error)?;
            diesel::insert_into(schema::tenant_codes::table)
                .values(NewTenantCode {
                    code: code.clone(),
                    tenant_id,
                    created_by: actor_id,
                })
                .execute(&mut *conn)
                .await
                .map_err(map_create_error)?;
            Ok(TenantSummary {
                tenant,
                code: Some(code),
            })
        })
        .await?;
    info!(
        platform_admin_id = %actor_id,
        tenant_id = %tenant_id,
        tenant_name = %summary.tenant.name,
        "平台管理员已自动生成并替换租户码"
    );
    Ok(summary)
}

pub async fn revoke_code(
    conn: &mut AsyncPgConnection,
    tenant_id: Uuid,
    actor_id: Uuid,
) -> AppResult<()> {
    let deleted = conn
        .transaction::<usize, AppError, _>(async |conn| {
            schema::tenants::table
                .filter(schema::tenants::id.eq(tenant_id))
                .for_update()
                .select(schema::tenants::id)
                .first::<Uuid>(&mut *conn)
                .await
                .map_err(|source| match source {
                    diesel::result::Error::NotFound => AppError::BadRequest {
                        message: format!("租户不存在: {tenant_id}"),
                    },
                    source => db_error(source),
                })?;
            diesel::delete(
                schema::tenant_codes::table.filter(schema::tenant_codes::tenant_id.eq(tenant_id)),
            )
            .execute(&mut *conn)
            .await
            .map_err(db_error)
        })
        .await?;
    if deleted == 0 {
        warn!(platform_admin_id = %actor_id, tenant_id = %tenant_id, "平台管理员撤销不存在的租户码，保持幂等");
    } else {
        info!(platform_admin_id = %actor_id, tenant_id = %tenant_id, "平台管理员已撤销租户码");
    }
    Ok(())
}

fn map_create_error(source: diesel::result::Error) -> AppError {
    match source {
        diesel::result::Error::DatabaseError(
            diesel::result::DatabaseErrorKind::UniqueViolation,
            information,
        ) => AppError::BadRequest {
            message: format!("租户名称或租户码已存在: {}", information.message()),
        },
        source => db_error(source),
    }
}

fn db_error(source: diesel::result::Error) -> AppError {
    AppError::DbQuery {
        message: source.to_string(),
    }
}
