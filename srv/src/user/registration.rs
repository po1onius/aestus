use diesel::prelude::*;
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl};
use rand::RngExt;
use sha2::{Digest, Sha256};
use tracing::{info, warn};
use uuid::Uuid;

use crate::{
    err::{AppError, AppResult},
    state::AppState,
    tenant,
};

use super::{
    credential::{hash_password, validate_registration_password},
    model::{
        USER_ROLE_PLATFORM_ADMIN, USER_ROLE_TENANT_OWNER, USER_ROLE_TENANT_USER, User,
        schema::users,
    },
    quota::validate_user_quota,
    repository::{create_user, find_by_email, find_by_username},
};

const EMAIL_CODE_DIGITS: u32 = 6;
const EMAIL_ADDRESS_MAX_BYTES: usize = 254;
const USERNAME_MIN_CHARS: usize = 1;
const USERNAME_MAX_CHARS: usize = 32;
const USERNAME_MAX_BYTES: usize = 128;
const EMAIL_CODE_MAX_ATTEMPTS: i64 = 5;

/// 租户创建事务使用的 owner 字段；密码哈希在开启数据库事务前完成。
pub(crate) struct PreparedTenantOwner {
    username: String,
    email: String,
    password_hash: String,
}

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
            .filter(users::tenant_id.eq(&tenant.id))
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
    tenant_id: String,
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

pub(crate) async fn prepare_tenant_owner(
    tenant_id: &str,
    password: String,
) -> AppResult<PreparedTenantOwner> {
    let username = normalize_username(tenant_id)?;
    let email = normalize_email(&format!("{username}@aes.tus"))?;
    let password_hash = hash_password(password).await?;

    info!(
        username,
        email, "平台管理员创建租户时的 owner 字段已完成校验和归一化"
    );
    Ok(PreparedTenantOwner {
        username,
        email,
        password_hash,
    })
}

pub(crate) async fn create_prepared_tenant_owner(
    conn: &mut AsyncPgConnection,
    tenant_id: String,
    owner: PreparedTenantOwner,
) -> AppResult<User> {
    create_user(
        conn,
        Some(tenant_id),
        owner.username,
        owner.email,
        owner.password_hash,
        USER_ROLE_TENANT_OWNER.to_owned(),
        0,
        true,
    )
    .await
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
