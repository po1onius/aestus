use std::collections::{BTreeMap, BTreeSet};

use diesel::prelude::*;
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl};
use serde::Serialize;
use tracing::{info, warn};
use uuid::Uuid;

use crate::{
    err::{AppError, AppResult},
    provider::group::{ProviderGroup, schema::provider_groups},
};

use super::{
    User,
    model::{USER_ROLE_TENANT_USER, schema::users},
};

pub mod schema {
    diesel::table! {
        tenant_user_group_grants (user_id, group_id) {
            tenant_id -> Text,
            user_id -> Uuid,
            group_id -> Uuid,
            granted_by -> Uuid,
            created_at -> Timestamptz,
            updated_at -> Timestamptz,
        }
    }

    diesel::table! {
        tenant_user_group_permissions (user_id, group_id, permission) {
            tenant_id -> Text,
            user_id -> Uuid,
            group_id -> Uuid,
            permission -> Text,
            granted_by -> Uuid,
            created_at -> Timestamptz,
        }
    }

    diesel::allow_tables_to_appear_in_same_query!(
        tenant_user_group_grants,
        tenant_user_group_permissions,
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum GroupPermission {
    AccountView,
    AccountQuotaView,
    AccountResetView,
    AccountResetConsume,
    AccountOverrideView,
    AccountOverrideUpdate,
    OfficialApiKeyView,
    OfficialApiKeyOverrideView,
    OfficialApiKeyOverrideUpdate,
}

impl GroupPermission {
    pub const ALL: [Self; 9] = [
        Self::AccountView,
        Self::AccountQuotaView,
        Self::AccountResetView,
        Self::AccountResetConsume,
        Self::AccountOverrideView,
        Self::AccountOverrideUpdate,
        Self::OfficialApiKeyView,
        Self::OfficialApiKeyOverrideView,
        Self::OfficialApiKeyOverrideUpdate,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AccountView => "account.view",
            Self::AccountQuotaView => "account.quota.view",
            Self::AccountResetView => "account.reset.view",
            Self::AccountResetConsume => "account.reset.consume",
            Self::AccountOverrideView => "account.override.view",
            Self::AccountOverrideUpdate => "account.override.update",
            Self::OfficialApiKeyView => "official_api_key.view",
            Self::OfficialApiKeyOverrideView => "official_api_key.override.view",
            Self::OfficialApiKeyOverrideUpdate => "official_api_key.override.update",
        }
    }

    fn parse(value: &str) -> AppResult<Self> {
        Self::ALL
            .into_iter()
            .find(|permission| permission.as_str() == value)
            .ok_or_else(|| AppError::BadRequest {
                message: format!("未知的分组权限: {value}"),
            })
    }

    fn prerequisites(self) -> &'static [Self] {
        match self {
            Self::AccountView | Self::OfficialApiKeyView => &[],
            Self::AccountQuotaView | Self::AccountResetView | Self::AccountOverrideView => {
                &[Self::AccountView]
            }
            // 应用重置后现有流程会立即刷新额度并清理已恢复的 quota_limited 调度状态，
            // 因此执行者必须同时具备读取额度和读取重置记录的能力。
            Self::AccountResetConsume => &[
                Self::AccountView,
                Self::AccountQuotaView,
                Self::AccountResetView,
            ],
            Self::AccountOverrideUpdate => &[Self::AccountView, Self::AccountOverrideView],
            Self::OfficialApiKeyOverrideView => &[Self::OfficialApiKeyView],
            Self::OfficialApiKeyOverrideUpdate => {
                &[Self::OfficialApiKeyView, Self::OfficialApiKeyOverrideView]
            }
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct UserGroupGrant {
    pub group_id: Uuid,
    pub permissions: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct GroupGrantInput {
    pub group_id: Uuid,
    pub permissions: Vec<String>,
}

#[derive(Insertable)]
#[diesel(table_name = schema::tenant_user_group_grants)]
struct NewGroupGrant {
    tenant_id: String,
    user_id: Uuid,
    group_id: Uuid,
    granted_by: Uuid,
}

#[derive(Insertable)]
#[diesel(table_name = schema::tenant_user_group_permissions)]
struct NewGroupPermission {
    tenant_id: String,
    user_id: Uuid,
    group_id: Uuid,
    permission: String,
    granted_by: Uuid,
}

pub async fn list_for_current_user(
    conn: &mut AsyncPgConnection,
    user: &User,
) -> AppResult<Vec<UserGroupGrant>> {
    if user.is_tenant_owner() {
        return Ok(Vec::new());
    }
    let tenant_id = user.tenant_id.clone().ok_or(AppError::Forbidden)?;
    list_for_user(conn, tenant_id, user.id).await
}

pub async fn list_for_managed_user(
    conn: &mut AsyncPgConnection,
    tenant_id: String,
    user_id: Uuid,
) -> AppResult<Vec<UserGroupGrant>> {
    require_managed_user(conn, tenant_id.clone(), user_id).await?;
    list_for_user(conn, tenant_id, user_id).await
}

async fn list_for_user(
    conn: &mut AsyncPgConnection,
    tenant_id: String,
    user_id: Uuid,
) -> AppResult<Vec<UserGroupGrant>> {
    use schema::{
        tenant_user_group_grants as grants, tenant_user_group_permissions as permissions,
    };

    let group_ids = grants::table
        .filter(grants::tenant_id.eq(tenant_id.clone()))
        .filter(grants::user_id.eq(user_id))
        .order(grants::group_id.asc())
        .select(grants::group_id)
        .load::<Uuid>(&mut *conn)
        .await
        .map_err(db_error)?;
    let permission_rows = permissions::table
        .filter(permissions::tenant_id.eq(tenant_id.clone()))
        .filter(permissions::user_id.eq(user_id))
        .order((permissions::group_id.asc(), permissions::permission.asc()))
        .select((permissions::group_id, permissions::permission))
        .load::<(Uuid, String)>(&mut *conn)
        .await
        .map_err(db_error)?;
    let mut by_group = BTreeMap::<Uuid, Vec<String>>::new();
    for (group_id, permission) in permission_rows {
        by_group.entry(group_id).or_default().push(permission);
    }
    Ok(group_ids
        .into_iter()
        .map(|group_id| UserGroupGrant {
            group_id,
            permissions: by_group.remove(&group_id).unwrap_or_default(),
        })
        .collect())
}

pub async fn replace_for_managed_user(
    conn: &mut AsyncPgConnection,
    tenant_id: String,
    user_id: Uuid,
    actor_id: Uuid,
    inputs: Vec<GroupGrantInput>,
) -> AppResult<Vec<UserGroupGrant>> {
    let normalized = normalize_inputs(inputs)?;
    let requested_group_ids = normalized.keys().copied().collect::<Vec<_>>();

    conn.transaction::<(), AppError, _>(async |conn| {
        let target = users::table
            .filter(users::id.eq(user_id))
            .filter(users::tenant_id.eq(Some(tenant_id.clone())))
            .for_update()
            .select(User::as_select())
            .first::<User>(&mut *conn)
            .await
            .map_err(|source| match source {
                diesel::result::Error::NotFound => AppError::BadRequest {
                    message: format!("普通租户用户不存在: {user_id}"),
                },
                source => db_error(source),
            })?;
        if target.role != USER_ROLE_TENANT_USER {
            return Err(AppError::BadRequest {
                message: "只能为普通租户用户配置分组授权".to_owned(),
            });
        }

        if !requested_group_ids.is_empty() {
            let groups = provider_groups::table
                .filter(provider_groups::tenant_id.eq(tenant_id.clone()))
                .filter(provider_groups::id.eq_any(&requested_group_ids))
                .order(provider_groups::id.asc())
                .for_update()
                .select(ProviderGroup::as_select())
                .load::<ProviderGroup>(&mut *conn)
                .await
                .map_err(db_error)?;
            if groups.len() != requested_group_ids.len() {
                return Err(AppError::BadRequest {
                    message: "部分 Provider 分组不存在或不属于当前租户".to_owned(),
                });
            }
        }

        use schema::{
            tenant_user_group_grants as grants, tenant_user_group_permissions as permissions,
        };
        diesel::delete(
            permissions::table
                .filter(permissions::tenant_id.eq(tenant_id.clone()))
                .filter(permissions::user_id.eq(user_id)),
        )
        .execute(&mut *conn)
        .await
        .map_err(db_error)?;
        diesel::delete(
            grants::table
                .filter(grants::tenant_id.eq(tenant_id.clone()))
                .filter(grants::user_id.eq(user_id)),
        )
        .execute(&mut *conn)
        .await
        .map_err(db_error)?;

        let grant_rows = requested_group_ids
            .iter()
            .map(|group_id| NewGroupGrant {
                tenant_id: tenant_id.clone(),
                user_id,
                group_id: *group_id,
                granted_by: actor_id,
            })
            .collect::<Vec<_>>();
        if !grant_rows.is_empty() {
            diesel::insert_into(grants::table)
                .values(&grant_rows)
                .execute(&mut *conn)
                .await
                .map_err(db_error)?;
        }
        let permission_rows = normalized
            .iter()
            .flat_map(|(group_id, group_permissions)| {
                group_permissions
                    .iter()
                    .map(|permission| NewGroupPermission {
                        tenant_id: tenant_id.clone(),
                        user_id,
                        group_id: *group_id,
                        permission: permission.as_str().to_owned(),
                        granted_by: actor_id,
                    })
            })
            .collect::<Vec<_>>();
        if !permission_rows.is_empty() {
            diesel::insert_into(permissions::table)
                .values(&permission_rows)
                .execute(&mut *conn)
                .await
                .map_err(db_error)?;
        }
        Ok(())
    })
    .await?;

    let after = list_for_user(conn, tenant_id.clone(), user_id).await?;
    info!(
        actor_user_id = %actor_id,
        target_user_id = %user_id,
        tenant_id = %tenant_id,
        current_grants = ?after,
        "租户 owner 已原子替换普通用户的 Provider 分组授权"
    );
    Ok(after)
}

pub async fn require_group_grant(
    conn: &mut AsyncPgConnection,
    user: &User,
    group_id: Uuid,
) -> AppResult<()> {
    if user.is_tenant_owner() {
        return Ok(());
    }
    let tenant_id = user.tenant_id.clone().ok_or(AppError::Forbidden)?;
    use schema::tenant_user_group_grants as grants;
    let granted = grants::table
        .filter(grants::tenant_id.eq(tenant_id.clone()))
        .filter(grants::user_id.eq(user.id))
        .filter(grants::group_id.eq(group_id))
        .select(grants::group_id)
        .first::<Uuid>(conn)
        .await
        .optional()
        .map_err(db_error)?
        .is_some();
    if granted {
        return Ok(());
    }
    warn!(user_id = %user.id, tenant_id = %tenant_id, provider_group_id = %group_id, "普通用户未获得 Provider 分组授权");
    Err(AppError::Forbidden)
}

pub async fn require_permission(
    conn: &mut AsyncPgConnection,
    user: &User,
    group_id: Option<Uuid>,
    permission: GroupPermission,
) -> AppResult<()> {
    if user.is_tenant_owner() {
        return Ok(());
    }
    let tenant_id = user.tenant_id.clone().ok_or(AppError::Forbidden)?;
    let Some(group_id) = group_id else {
        return Err(AppError::Forbidden);
    };
    require_group_grant(conn, user, group_id).await?;
    let mut required = permission.prerequisites().to_vec();
    required.push(permission);
    let required_names = required
        .iter()
        .map(|permission| permission.as_str())
        .collect::<Vec<_>>();
    use schema::tenant_user_group_permissions as permissions;
    let granted_count = permissions::table
        .filter(permissions::tenant_id.eq(tenant_id.clone()))
        .filter(permissions::user_id.eq(user.id))
        .filter(permissions::group_id.eq(group_id))
        .filter(permissions::permission.eq_any(&required_names))
        .select(permissions::permission)
        .load::<String>(conn)
        .await
        .map_err(db_error)?
        .into_iter()
        .collect::<BTreeSet<_>>()
        .len();
    if granted_count == required_names.len() {
        return Ok(());
    }
    warn!(
        user_id = %user.id,
        tenant_id = %tenant_id,
        provider_group_id = %group_id,
        required_permission = permission.as_str(),
        "普通用户缺少组内资源权限"
    );
    Err(AppError::Forbidden)
}

pub async fn group_ids_with_permission(
    conn: &mut AsyncPgConnection,
    user: &User,
    permission: GroupPermission,
) -> AppResult<Option<Vec<Uuid>>> {
    if user.is_tenant_owner() {
        return Ok(None);
    }
    let tenant_id = user.tenant_id.clone().ok_or(AppError::Forbidden)?;
    use schema::{
        tenant_user_group_grants as grants, tenant_user_group_permissions as permissions,
    };
    let group_ids = permissions::table
        .inner_join(
            grants::table.on(grants::tenant_id
                .eq(permissions::tenant_id)
                .and(grants::user_id.eq(permissions::user_id))
                .and(grants::group_id.eq(permissions::group_id))),
        )
        .filter(permissions::tenant_id.eq(tenant_id.clone()))
        .filter(permissions::user_id.eq(user.id))
        .filter(permissions::permission.eq(permission.as_str()))
        .order(permissions::group_id.asc())
        .select(permissions::group_id)
        .load::<Uuid>(conn)
        .await
        .map_err(db_error)?;
    Ok(Some(group_ids))
}

pub async fn granted_group_ids(
    conn: &mut AsyncPgConnection,
    user: &User,
) -> AppResult<Option<Vec<Uuid>>> {
    if user.is_tenant_owner() {
        return Ok(None);
    }
    let tenant_id = user.tenant_id.clone().ok_or(AppError::Forbidden)?;
    use schema::tenant_user_group_grants as grants;
    let group_ids = grants::table
        .filter(grants::tenant_id.eq(tenant_id.clone()))
        .filter(grants::user_id.eq(user.id))
        .order(grants::group_id.asc())
        .select(grants::group_id)
        .load::<Uuid>(conn)
        .await
        .map_err(db_error)?;
    Ok(Some(group_ids))
}

fn normalize_inputs(
    inputs: Vec<GroupGrantInput>,
) -> AppResult<BTreeMap<Uuid, BTreeSet<GroupPermission>>> {
    let mut normalized = BTreeMap::new();
    for input in inputs {
        if normalized.contains_key(&input.group_id) {
            return Err(AppError::BadRequest {
                message: format!("Provider 分组授权重复: {}", input.group_id),
            });
        }
        let permissions = input
            .permissions
            .iter()
            .map(|permission| GroupPermission::parse(permission))
            .collect::<AppResult<BTreeSet<_>>>()?;
        for permission in &permissions {
            if let Some(missing) = permission
                .prerequisites()
                .iter()
                .find(|required| !permissions.contains(required))
            {
                return Err(AppError::BadRequest {
                    message: format!("权限 {} 依赖权限 {}", permission.as_str(), missing.as_str()),
                });
            }
        }
        normalized.insert(input.group_id, permissions);
    }
    Ok(normalized)
}

async fn require_managed_user(
    conn: &mut AsyncPgConnection,
    tenant_id: String,
    user_id: Uuid,
) -> AppResult<User> {
    let target = users::table
        .filter(users::id.eq(user_id))
        .filter(users::tenant_id.eq(Some(tenant_id.clone())))
        .select(User::as_select())
        .first::<User>(conn)
        .await
        .map_err(|source| match source {
            diesel::result::Error::NotFound => AppError::BadRequest {
                message: format!("普通租户用户不存在: {user_id}"),
            },
            source => db_error(source),
        })?;
    if target.role != USER_ROLE_TENANT_USER {
        return Err(AppError::BadRequest {
            message: "只能查看或配置普通租户用户的分组授权".to_owned(),
        });
    }
    Ok(target)
}

fn db_error(source: diesel::result::Error) -> AppError {
    AppError::DbQuery {
        message: source.to_string(),
    }
}
