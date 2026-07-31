use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{get, put},
};
use serde::Deserialize;
use uuid::Uuid;

use crate::{
    err::{AdminResult, AppError},
    http::dash::{
        auth,
        pagination::{ListPage, ListPageQuery},
    },
    state::AppState,
    user::{self, PublicUser},
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateUserQuotaRequest {
    quota: i64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateUserStatusRequest {
    enabled: bool,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list_users))
        .route("/{id}/quota", put(update_user_quota))
        .route("/{id}/status", put(update_user_status))
}

async fn list_users(
    State(state): State<AppState>,
    _admin: auth::AdminUser,
    Query(query): Query<ListPageQuery>,
) -> AdminResult<Json<ListPage<PublicUser>>> {
    let page = query.normalize()?;
    let mut conn = state.db_conn().await?;
    let items = user::list(&mut conn, page.query_limit(), page.offset())
        .await?
        .into_iter()
        .map(PublicUser::from)
        .collect();

    Ok(Json(page.finish(items)))
}

async fn update_user_quota(
    State(state): State<AppState>,
    _admin: auth::AdminUser,
    Path(id): Path<Uuid>,
    Json(payload): Json<UpdateUserQuotaRequest>,
) -> AdminResult<Json<PublicUser>> {
    let mut conn = state.db_conn().await?;
    let user = user::update_quota(&mut conn, id, payload.quota).await?;

    Ok(Json(user.into()))
}

async fn update_user_status(
    State(state): State<AppState>,
    auth::AdminUser(current_user): auth::AdminUser,
    Path(id): Path<Uuid>,
    Json(payload): Json<UpdateUserStatusRequest>,
) -> AdminResult<Json<PublicUser>> {
    if current_user.id == id && !payload.enabled {
        return Err(AppError::BadRequest {
            message: "不能禁用当前登录的 admin 用户".to_owned(),
        }
        .into());
    }

    let mut conn = state.db_conn().await?;
    let user = user::update_status(&mut conn, id, payload.enabled).await?;

    Ok(Json(user.into()))
}
