//! `/api/certificates` handlers.

use super::{
    error::{ApiError, ApiResult},
    json::JsonBody,
};
use crate::{
    certificates::{CertificateView, NewCertificate},
    state::AppState,
};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, post},
};
use std::sync::Arc;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/certificates", get(list).post(create))
        .route("/certificates/{id}", delete(remove))
        .route("/certificates/{id}/renew", post(renew))
}

async fn list(State(state): State<Arc<AppState>>) -> ApiResult<Json<Vec<CertificateView>>> {
    Ok(Json(state.certificates.list().map_err(ApiError::internal)?))
}

async fn create(
    State(state): State<Arc<AppState>>,
    JsonBody(request): JsonBody<NewCertificate>,
) -> ApiResult<(StatusCode, Json<CertificateView>)> {
    let created = state.certificates.create(request).await?;
    Ok((StatusCode::CREATED, Json(created)))
}

async fn renew(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<CertificateView>> {
    Ok(Json(state.certificates.renew(&id).await?))
}

async fn remove(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    state.delete_certificate(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}
