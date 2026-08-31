use bcrypt::{DEFAULT_COST, hash, verify};
use chrono::{DateTime, Duration, Utc};
use diesel::dsl::case_when;
use diesel::prelude::*;
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};
use rand::RngExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{info, warn};
use uuid::Uuid;

use crate::{
    err::{AppError, AppResult},
    infra::db::{self, DbPool},
    model::{
        NewUser, USER_ROLE_PLATFORM_ADMIN, USER_ROLE_TENANT_OWNER, USER_ROLE_TENANT_USER, User,
        UserStatusPatch, is_valid_user_role, schema::users,
    },
    request_event::TokenUsage,
    state::AppState,
    tenant,
};

const EMAIL_CODE_DIGITS: u32 = 6;
const EMAIL_ADDRESS_MAX_BYTES: usize = 254;
const USERNAME_MIN_CHARS: usize = 1;
const USERNAME_MAX_CHARS: usize = 32;
const USERNAME_MAX_BYTES: usize = 128;
const PASSWORD_MIN_CHARS: usize = 8;
const PASSWORD_MAX_BYTES: usize = 72;
const EMAIL_CODE_MAX_ATTEMPTS: i64 = 5;
/// JavaScript `number` 能精确表示的最大整数。Dashboard JSON 直接以数字返回额度，因此
/// 持久层也必须使用同一上限，避免浏览器收到已经发生舍入的额度。
pub const MAX_USER_QUOTA: i64 = 9_007_199_254_740_991;

const VERIFY_EMAIL_CODE_LUA: &str = r#"
local saved_hash = redis.call('GET', KEYS[1])
if not saved_hash then
    return 0
end
if saved_hash == ARGV[1] then
    redis.call('DEL', KEYS[1])
    redis.call('DEL', KEYS[2])
    return 1
end
local attempts = redis.call('INCR', KEYS[2])
if attempts == 1 then
    local code_ttl = redis.call('TTL', KEYS[1])
    if code_ttl > 0 then
        redis.call('EXPIRE', KEYS[2], code_ttl)
    end
end
if attempts >= tonumber(ARGV[2]) then
    redis.call('DEL', KEYS[1])
    redis.call('DEL', KEYS[2])
    return -1
end
return -2
"#;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JwtClaims {
    pub sub: Uuid,
    pub role: String,
    pub exp: i64,
    pub iat: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct PublicUser {
    pub id: Uuid,
    pub tenant_id: Option<Uuid>,
    pub username: String,
    pub email: String,
    pub role: String,
    pub quota: i64,
    pub email_verified: bool,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub disabled_at: Option<DateTime<Utc>>,
}

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

impl From<User> for PublicUser {
    fn from(user: User) -> Self {
        Self {
            id: user.id,
            tenant_id: user.tenant_id,
            username: user.username,
            email: user.email,
            role: user.role,
            quota: user.quota,
            email_verified: user.email_verified,
            enabled: user.enabled,
            created_at: user.created_at,
            updated_at: user.updated_at,
            disabled_at: user.disabled_at,
        }
    }
}

/// 启动时初始化平台管理员用户。
///
/// 如果平台管理员邮箱已经存在，只记录日志不覆盖密码和额度，避免服务重启时误改人为调整。
pub async fn bootstrap_admin(state: &AppState) -> AppResult<()> {
    let username = normalize_username(&state.config().admin_username)?;
    let email = normalize_email(&state.config().admin_email)?;
    // 即使 admin 已存在也校验初始化配置，避免错误额度或密码边界被静默保留到下一次
    // 全新数据库初始化时才暴露。
    validate_user_quota(state.config().admin_initial_quota)?;
    validate_registration_password(&state.config().admin_password)?;
    let mut conn = state.db_conn().await?;

    if let Some(user) = find_by_email(&mut conn, &email).await? {
        info!(user_id = %user.id, username = %user.username, email, "平台管理员用户已存在，跳过启动初始化");
        return Ok(());
    }
    if find_by_username(&mut conn, &username).await?.is_some() {
        return Err(AppError::BadRequest {
            message: format!("平台管理员用户名已被其他用户占用: {username}"),
        });
    }

    let password_hash = hash_password(state.config().admin_password.clone()).await?;
    let user = create_user(
        &mut conn,
        None,
        username,
        email,
        password_hash,
        USER_ROLE_PLATFORM_ADMIN.to_owned(),
        state.config().admin_initial_quota,
        true,
    )
    .await?;

    info!(
        user_id = %user.id,
        username = %user.username,
        email = %user.email,
        quota = user.quota,
        "平台管理员用户初始化完成"
    );

    Ok(())
}

pub async fn register_with_tenant_code(
    conn: &mut AsyncPgConnection,
    tenant_code: String,
    username: String,
    email: String,
    password: String,
) -> AppResult<User> {
    let username = normalize_username(&username)?;
    let email = normalize_email(&email)?;
    let tenant_code = tenant::normalize_code(tenant_code)?;
    let password_hash = hash_password(password).await?;

    conn.transaction::<User, AppError, _>(async |conn| {
        let tenant = tenant::find_enabled_by_code_for_update(&mut *conn, &tenant_code)
            .await?
            .ok_or_else(|| AppError::BadRequest {
                message: "租户码无效或对应租户已停用".to_owned(),
            })?;
        let owner_exists = users::table
            .filter(users::tenant_id.eq(tenant.id))
            .filter(users::role.eq(USER_ROLE_TENANT_OWNER))
            .select(users::id)
            .first::<Uuid>(&mut *conn)
            .await
            .optional()
            .map_err(|source| AppError::DbQuery {
                message: source.to_string(),
            })?
            .is_some();
        let role = if owner_exists {
            USER_ROLE_TENANT_USER
        } else {
            USER_ROLE_TENANT_OWNER
        };
        create_user(
            &mut *conn,
            Some(tenant.id),
            username,
            email,
            password_hash,
            role.to_owned(),
            0,
            true,
        )
        .await
    })
    .await
}

/// 由租户 owner 直接创建普通用户。
///
/// 租户 owner 已经在受保护的 Dashboard 中完成身份校验，因此该流程不发送或校验邮箱验证码；
/// 邮箱留空时使用归一化后的用户名生成稳定的站内默认地址。最终仍复用普通用户创建逻辑，
/// 使用户名、邮箱、密码边界以及数据库唯一冲突在所有创建入口保持一致。
pub async fn create_owner_managed_user(
    conn: &mut AsyncPgConnection,
    tenant_id: Uuid,
    username: String,
    email: Option<String>,
    password: String,
) -> AppResult<User> {
    let username = normalize_username(&username)?;
    let (email, default_email_used) = match email.as_deref().map(str::trim) {
        Some(email) if !email.is_empty() => (normalize_email(email)?, false),
        _ => (normalize_email(&format!("{username}@aes.tus"))?, true),
    };

    info!(
        username,
        email, default_email_used, "租户 owner 创建用户请求已完成字段归一化"
    );

    create_user(
        conn,
        Some(tenant_id),
        username,
        email,
        hash_password(password).await?,
        USER_ROLE_TENANT_USER.to_owned(),
        0,
        true,
    )
    .await
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
    .set((dsl::quota.eq(quota), dsl::updated_at.eq(Utc::now())))
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
        disabled_at: if enabled { None } else { Some(Utc::now()) },
        updated_at: Utc::now(),
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

pub async fn verify_password(password: String, password_hash: String) -> AppResult<bool> {
    // bcrypt 只使用前 72 字节。登录时必须先拒绝更长输入，不能让两个不同后缀的密码
    // 因为底层静默截断而得到相同校验结果。
    if password.len() > PASSWORD_MAX_BYTES {
        burn_dummy_password_verification().await?;
        return Ok(false);
    }

    tokio::task::spawn_blocking(move || verify(password, &password_hash))
        .await
        .map_err(|source| AppError::Startup {
            message: format!("bcrypt 密码校验任务异常结束: {source}"),
        })?
        .map_err(|source| AppError::Startup {
            message: format!("bcrypt 密码哈希格式无效: {source}"),
        })
}

/// 为不存在用户或明显越界密码消耗一次与正常登录同量级的 bcrypt 工作。
///
/// 这不是服务内限流；入口仍需由 Nginx 限流。它只避免公开登录接口因是否执行 bcrypt
/// 呈现明显的账号存在性时序差异。
pub async fn burn_dummy_password_verification() -> AppResult<()> {
    tokio::task::spawn_blocking(|| hash("dashboard-login-dummy-password", DEFAULT_COST))
        .await
        .map_err(|source| AppError::Startup {
            message: format!("bcrypt 虚拟校验任务异常结束: {source}"),
        })?
        .map(|_| ())
        .map_err(|source| AppError::Startup {
            message: format!("bcrypt 虚拟校验失败: {source}"),
        })
}

pub fn issue_jwt(state: &AppState, user: &User) -> AppResult<String> {
    let now = Utc::now();
    let expires_at = now + Duration::seconds(state.config().jwt_ttl_seconds as i64);
    let claims = JwtClaims {
        sub: user.id,
        role: user.role.clone(),
        iat: now.timestamp(),
        exp: expires_at.timestamp(),
    };

    encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(state.config().jwt_secret.as_bytes()),
    )
    .map_err(|source| AppError::InvalidConfig {
        key: "AESTUS_JWT_SECRET",
        value: "<redacted>".to_owned(),
        source: Box::new(source),
    })
}

pub fn decode_jwt(state: &AppState, token: &str) -> AppResult<JwtClaims> {
    decode::<JwtClaims>(
        token,
        &DecodingKey::from_secret(state.config().jwt_secret.as_bytes()),
        &Validation::new(Algorithm::HS256),
    )
    .map(|data| data.claims)
    .map_err(|source| {
        warn!(error = %source, "Dashboard JWT 解析失败");
        AppError::InvalidDashboardToken
    })
}

pub async fn send_register_email_code(state: &AppState, email: String) -> AppResult<()> {
    let email = normalize_email(&email)?;
    let code = generate_email_code();
    let mut redis = state.redis();
    let cooldown_key = register_email_code_cooldown_key(&email);
    let code_key = register_email_code_key(&email);

    // 单条 SET NX EX 同时建立冷却和 TTL，避免 SETNX 成功后进程退出留下永久冷却 key。
    let cooldown_acquired: Option<String> = redis::cmd("SET")
        .arg(&cooldown_key)
        .arg("1")
        .arg("NX")
        .arg("EX")
        .arg(state.config().email_code_cooldown_seconds.max(1))
        .query_async(&mut redis)
        .await
        .map_err(|source| AppError::Redis {
            message: source.to_string(),
        })?;
    if cooldown_acquired.is_none() {
        return Err(AppError::BadRequest {
            message: "验证码发送过于频繁，请稍后再试".to_owned(),
        });
    }

    let code_hash = hash_email_code(&email, &code);
    let attempt_key = register_email_code_attempt_key(&email);
    let _: () = redis::pipe()
        .atomic()
        .cmd("SETEX")
        .arg(&code_key)
        .arg(state.config().email_code_ttl_seconds.max(1))
        .arg(code_hash)
        .ignore()
        .cmd("DEL")
        .arg(&attempt_key)
        .ignore()
        .query_async(&mut redis)
        .await
        .map_err(|source| AppError::Redis {
            message: source.to_string(),
        })?;

    if let Err(error) = state.email_client().send_register_code(&email, &code).await {
        // 邮件没有提交成功时清理本次验证码和冷却，让用户修正地址或基础设施后立即重试。
        let cleanup_result: Result<(), redis::RedisError> = redis::pipe()
            .atomic()
            .cmd("DEL")
            .arg(&code_key)
            .ignore()
            .cmd("DEL")
            .arg(&attempt_key)
            .ignore()
            .cmd("DEL")
            .arg(&cooldown_key)
            .ignore()
            .query_async(&mut redis)
            .await;
        if let Err(cleanup_error) = cleanup_result {
            warn!(email, error = %cleanup_error, "注册邮件发送失败后清理 Redis 验证码状态失败");
        }
        return Err(error);
    }
    info!(email, "注册邮箱验证码已写入 Redis 并发送");
    Ok(())
}

pub async fn verify_register_email_code(
    state: &AppState,
    email: &str,
    code: &str,
) -> AppResult<()> {
    let email = normalize_email(email)?;
    let normalized_code = code.trim();
    if normalized_code.len() != EMAIL_CODE_DIGITS as usize
        || !normalized_code.chars().all(|item| item.is_ascii_digit())
    {
        return Err(AppError::BadRequest {
            message: "验证码格式无效".to_owned(),
        });
    }

    let code_key = register_email_code_key(&email);
    let attempt_key = register_email_code_attempt_key(&email);
    let mut redis = state.redis();
    // 只有匹配成功才删除验证码；错误尝试在同一 Lua 脚本中计数并继承验证码 TTL，既避免
    // 一次错误输入使正确验证码失效，也限制六位验证码的在线猜测次数。
    let result: i64 = redis::cmd("EVAL")
        .arg(VERIFY_EMAIL_CODE_LUA)
        .arg(2)
        .arg(&code_key)
        .arg(&attempt_key)
        .arg(hash_email_code(&email, normalized_code))
        .arg(EMAIL_CODE_MAX_ATTEMPTS)
        .query_async(&mut redis)
        .await
        .map_err(|source| AppError::Redis {
            message: source.to_string(),
        })?;

    match result {
        1 => {}
        0 => {
            return Err(AppError::BadRequest {
                message: "验证码不存在或已过期".to_owned(),
            });
        }
        -1 => {
            warn!(
                email,
                max_attempts = EMAIL_CODE_MAX_ATTEMPTS,
                "注册验证码错误次数达到上限"
            );
            return Err(AppError::BadRequest {
                message: "验证码错误次数过多，请重新获取".to_owned(),
            });
        }
        _ => {
            warn!(email, "注册验证码校验失败");
            return Err(AppError::BadRequest {
                message: "验证码错误".to_owned(),
            });
        }
    }

    info!(email, "注册验证码校验通过");
    Ok(())
}

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

pub fn normalize_email(email: &str) -> AppResult<String> {
    let email = email.trim().to_ascii_lowercase();
    if email.is_empty() || email.len() > EMAIL_ADDRESS_MAX_BYTES {
        return Err(AppError::BadRequest {
            message: "邮箱格式无效".to_owned(),
        });
    }
    let parsed =
        email
            .parse::<email_address::EmailAddress>()
            .map_err(|_| AppError::BadRequest {
                message: "邮箱格式无效".to_owned(),
            })?;
    if !parsed.display_part().is_empty() {
        return Err(AppError::BadRequest {
            message: "邮箱必须是纯地址，不能包含显示名称".to_owned(),
        });
    }
    let email = parsed.email().to_ascii_lowercase();
    if email.len() > EMAIL_ADDRESS_MAX_BYTES {
        return Err(AppError::BadRequest {
            message: "邮箱格式无效".to_owned(),
        });
    }
    Ok(email)
}

/// 用户名既是 Dashboard 展示名称，也是邮箱之外的登录标识。
///
/// 统一转为小写并限制为 Unicode 字母、数字、下划线和连字符，使数据库唯一约束与登录
/// 匹配拥有完全一致的语义；禁止 `@` 也避免用户名与邮箱登录分支产生歧义。
pub fn normalize_username(username: &str) -> AppResult<String> {
    let username = username.trim().to_lowercase();
    let char_count = username.chars().count();
    let valid_length = (USERNAME_MIN_CHARS..=USERNAME_MAX_CHARS).contains(&char_count)
        && username.len() <= USERNAME_MAX_BYTES;
    let mut chars = username.chars();
    let valid_first = chars.next().is_some_and(char::is_alphanumeric);
    let valid_rest =
        chars.all(|character| character.is_alphanumeric() || matches!(character, '_' | '-'));
    if !valid_length || !valid_first || !valid_rest {
        return Err(AppError::BadRequest {
            message: format!(
                "用户名必须为 {USERNAME_MIN_CHARS} 到 {USERNAME_MAX_CHARS} 个字符且不超过 {USERNAME_MAX_BYTES} 字节，只能包含字母、数字、下划线和连字符，并以字母或数字开头"
            ),
        });
    }
    Ok(username)
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

pub(crate) fn validate_registration_password(password: &str) -> AppResult<()> {
    if password.chars().count() < PASSWORD_MIN_CHARS {
        return Err(AppError::BadRequest {
            message: "密码长度不能少于 8 位".to_owned(),
        });
    }
    if password.len() > PASSWORD_MAX_BYTES {
        return Err(AppError::BadRequest {
            message: format!("密码 UTF-8 编码长度不能超过 {PASSWORD_MAX_BYTES} 字节"),
        });
    }
    Ok(())
}

pub(crate) fn validate_user_quota(quota: i64) -> AppResult<()> {
    if !(0..=MAX_USER_QUOTA).contains(&quota) {
        return Err(AppError::BadRequest {
            message: format!("用户额度必须在 0 到 {MAX_USER_QUOTA} 之间"),
        });
    }
    Ok(())
}

async fn hash_password(password: String) -> AppResult<String> {
    validate_registration_password(&password)?;
    tokio::task::spawn_blocking(move || hash(password, DEFAULT_COST))
        .await
        .map_err(|source| AppError::Startup {
            message: format!("bcrypt 密码哈希任务异常结束: {source}"),
        })?
        .map_err(|source| AppError::Startup {
            message: format!("bcrypt 密码哈希失败: {source}"),
        })
}

fn generate_email_code() -> String {
    let max = 10_u32.pow(EMAIL_CODE_DIGITS);
    let code = rand::rng().random_range(0..max);
    format!("{code:0width$}", width = EMAIL_CODE_DIGITS as usize)
}

fn hash_email_code(email: &str, code: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(email.as_bytes());
    hasher.update(b":");
    hasher.update(code.as_bytes());
    hex::encode(hasher.finalize())
}

fn register_email_code_key(email: &str) -> String {
    format!("register:email_code:{email}")
}

fn register_email_code_cooldown_key(email: &str) -> String {
    format!("register:email_code:cooldown:{email}")
}

fn register_email_code_attempt_key(email: &str) -> String {
    format!("register:email_code:attempts:{email}")
}
