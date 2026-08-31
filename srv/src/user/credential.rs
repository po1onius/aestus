use bcrypt::{DEFAULT_COST, hash, verify};
use chrono::{Duration, Utc};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use tracing::warn;
use uuid::Uuid;

use crate::{
    err::{AppError, AppResult},
    state::AppState,
};

use super::model::User;

const PASSWORD_MIN_CHARS: usize = 8;
const PASSWORD_MAX_BYTES: usize = 72;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JwtClaims {
    pub sub: Uuid,
    pub role: String,
    pub exp: i64,
    pub iat: i64,
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

pub(super) async fn hash_password(password: String) -> AppResult<String> {
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
