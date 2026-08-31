use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use tracing::info;
use uuid::Uuid;

use crate::err::{AppError, AppResult};

use super::{
    model::{
        NewUser, USER_ROLE_TENANT_USER, User, UserStatusPatch, is_valid_user_role, schema::users,
    },
    quota::validate_user_quota,
    registration::{normalize_email, normalize_username},
};

/// Dashboard 全局用量统计需要的最小用户快照。
///
/// 这里只投影聚合额度和 API Key 归属展示所需字段，不加载密码、状态时间等无关数据；
/// 汇总使用 `i128` 在统计层完成，避免多个合法 `BIGINT` 用户额度相加时溢出 `i64`。
#[derive(Debug)]
pub struct UserUsageSnapshot {
    pub id: Uuid,
    pub username: String,
    pub quota: i64,
    pub consumed_tokens: i64,
}

pub async fn create_user(
    conn: &mut AsyncPgConnection,
    tenant_id: Option<Uuid>,
    username: String,
    email: String,
    password_hash: String,
    role: String,
    quota: i64,
    email_verified: bool,
) -> AppResult<User> {
    use self::users::dsl;

    let username = normalize_username(&username)?;
    let email = normalize_email(&email)?;
    if !is_valid_user_role(&role) {
        return Err(AppError::BadRequest {
            message: format!("用户角色无效: {role}"),
        });
    }
    validate_user_quota(quota)?;

    let user = diesel::insert_into(dsl::users)
        .values(&NewUser {
            tenant_id,
            username,
            email,
            password_hash,
            role,
            quota,
            email_verified,
            enabled: true,
        })
        .returning(User::as_returning())
        .get_result::<User>(conn)
        .await
        .map_err(map_user_insert_error)?;

    info!(user_id = %user.id, username = %user.username, email = %user.email, role = %user.role, "用户已创建");
    Ok(user)
}

pub async fn find_by_username(
    conn: &mut AsyncPgConnection,
    username: &str,
) -> AppResult<Option<User>> {
    use self::users::dsl;

    let result = dsl::users
        .filter(dsl::username.eq(username))
        .select(User::as_select())
        .first::<User>(conn)
        .await;

    match result {
        Ok(user) => Ok(Some(user)),
        Err(diesel::result::Error::NotFound) => Ok(None),
        Err(source) => Err(AppError::DbQuery {
            message: source.to_string(),
        }),
    }
}

pub async fn find_by_email(conn: &mut AsyncPgConnection, email: &str) -> AppResult<Option<User>> {
    use self::users::dsl;

    let result = dsl::users
        .filter(dsl::email.eq(email))
        .select(User::as_select())
        .first::<User>(conn)
        .await;

    match result {
        Ok(user) => Ok(Some(user)),
        Err(diesel::result::Error::NotFound) => Ok(None),
        Err(source) => Err(AppError::DbQuery {
            message: source.to_string(),
        }),
    }
}

/// 使用登录标识查找用户。包含 `@` 的输入只按邮箱解释，其余输入只按用户名解释。
/// 格式不合法时返回未命中，让公开登录接口继续执行虚拟 bcrypt 并返回统一凭证错误。
pub async fn find_by_login_identifier(
    conn: &mut AsyncPgConnection,
    identifier: &str,
) -> AppResult<Option<User>> {
    if identifier.contains('@') {
        let Ok(email) = normalize_email(identifier) else {
            return Ok(None);
        };
        find_by_email(conn, &email).await
    } else {
        let Ok(username) = normalize_username(identifier) else {
            return Ok(None);
        };
        find_by_username(conn, &username).await
    }
}

pub async fn find_by_id(conn: &mut AsyncPgConnection, id: Uuid) -> AppResult<Option<User>> {
    use self::users::dsl;

    let result = dsl::users
        .filter(dsl::id.eq(id))
        .select(User::as_select())
        .first::<User>(conn)
        .await;

    match result {
        Ok(user) => Ok(Some(user)),
        Err(diesel::result::Error::NotFound) => Ok(None),
        Err(source) => Err(AppError::DbQuery {
            message: source.to_string(),
        }),
    }
}

pub async fn list_by_tenant(
    conn: &mut AsyncPgConnection,
    tenant_id: Uuid,
    limit: i64,
    offset: i64,
) -> AppResult<Vec<User>> {
    use self::users::dsl;

    dsl::users
        .filter(dsl::tenant_id.eq(tenant_id))
        .order((dsl::created_at.desc(), dsl::id.desc()))
        .limit(limit)
        .offset(offset)
        .select(User::as_select())
        .load::<User>(conn)
        .await
        .map_err(|source| AppError::DbQuery {
            message: source.to_string(),
        })
}

pub async fn list_usage_snapshots(
    conn: &mut AsyncPgConnection,
    tenant_id: Option<Uuid>,
) -> AppResult<Vec<UserUsageSnapshot>> {
    use self::users::dsl;

    let mut query = dsl::users.into_boxed();
    if let Some(tenant_id) = tenant_id {
        query = query.filter(dsl::tenant_id.eq(tenant_id));
    }
    query
        .select((dsl::id, dsl::username, dsl::quota, dsl::consumed_tokens))
        .load::<(Uuid, String, i64, i64)>(conn)
        .await
        .map(|rows| {
            rows.into_iter()
                .map(|(id, username, quota, consumed_tokens)| UserUsageSnapshot {
                    id,
                    username,
                    quota,
                    consumed_tokens,
                })
                .collect()
        })
        .map_err(|source| AppError::DbQuery {
            message: source.to_string(),
        })
}

pub async fn update_quota_for_tenant(
    conn: &mut AsyncPgConnection,
    tenant_id: Uuid,
    id: Uuid,
    quota: i64,
) -> AppResult<User> {
    use self::users::dsl;

    validate_user_quota(quota)?;

    let result = diesel::update(
        dsl::users
            .filter(dsl::id.eq(id))
            .filter(dsl::tenant_id.eq(tenant_id)),
    )
    .set((dsl::quota.eq(quota), dsl::updated_at.eq(chrono::Utc::now())))
    .returning(User::as_returning())
    .get_result::<User>(conn)
    .await;

    match result {
        Ok(user) => {
            info!(user_id = %user.id, username = %user.username, quota = user.quota, "用户 token 额度已更新");
            Ok(user)
        }
        Err(diesel::result::Error::NotFound) => Err(AppError::BadRequest {
            message: format!("用户不存在: {id}"),
        }),
        Err(source) => Err(AppError::DbQuery {
            message: source.to_string(),
        }),
    }
}

pub async fn update_status(
    conn: &mut AsyncPgConnection,
    tenant_id: Uuid,
    id: Uuid,
    enabled: bool,
) -> AppResult<User> {
    use self::users::dsl;

    let patch = UserStatusPatch {
        enabled,
        disabled_at: if enabled {
            None
        } else {
            Some(chrono::Utc::now())
        },
        updated_at: chrono::Utc::now(),
    };

    let result = diesel::update(
        dsl::users
            .filter(dsl::id.eq(id))
            .filter(dsl::tenant_id.eq(tenant_id))
            .filter(dsl::role.eq(USER_ROLE_TENANT_USER)),
    )
    .set(&patch)
    .returning(User::as_returning())
    .get_result::<User>(conn)
    .await;

    match result {
        Ok(user) => {
            info!(user_id = %user.id, username = %user.username, enabled = user.enabled, "用户启用状态已更新");
            Ok(user)
        }
        Err(diesel::result::Error::NotFound) => Err(AppError::BadRequest {
            message: format!("用户不存在: {id}"),
        }),
        Err(source) => Err(AppError::DbQuery {
            message: source.to_string(),
        }),
    }
}

fn map_user_insert_error(source: diesel::result::Error) -> AppError {
    if let diesel::result::Error::DatabaseError(
        diesel::result::DatabaseErrorKind::UniqueViolation,
        details,
    ) = &source
    {
        return match details.constraint_name() {
            Some("uq_users_username") => AppError::BadRequest {
                message: "用户名已被使用".to_owned(),
            },
            Some("users_email_key") => AppError::BadRequest {
                message: "邮箱已注册".to_owned(),
            },
            _ => AppError::DbQuery {
                message: source.to_string(),
            },
        };
    }
    AppError::DbQuery {
        message: source.to_string(),
    }
}
