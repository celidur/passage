//! `/api/routes` handlers.

use super::{
    error::{ApiError, ApiResult},
    json::JsonBody,
};
use crate::{
    routes::{Route, RouteInput},
    state::AppState,
    store::Change,
};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, put},
};
use std::sync::Arc;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/routes", get(list).post(create))
        .route("/routes/{id}", put(update).delete(remove))
}

async fn list(State(state): State<Arc<AppState>>) -> ApiResult<Json<Vec<Route>>> {
    Ok(Json(state.routes().map_err(ApiError::internal)?))
}

async fn create(
    State(state): State<Arc<AppState>>,
    JsonBody(mut input): JsonBody<RouteInput>,
) -> ApiResult<(StatusCode, Json<Route>)> {
    input.validate().map_err(ApiError::bad_request)?;
    let route = Route {
        id: uuid::Uuid::new_v4().to_string(),
        input,
    };
    state.apply(Change::Create(route.clone())).await?;
    Ok((StatusCode::CREATED, Json(route)))
}

async fn update(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    JsonBody(mut input): JsonBody<RouteInput>,
) -> ApiResult<Json<Route>> {
    input.validate().map_err(ApiError::bad_request)?;
    let route = Route { id, input };
    state.apply(Change::Update(route.clone())).await?;
    Ok(Json(route))
}

async fn remove(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    state.apply(Change::Delete(id)).await?;
    Ok(StatusCode::NO_CONTENT)
}
