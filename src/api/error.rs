use crate::state::UpdateError;
use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;

pub type ApiResult<T> = Result<T, ApiError>;

/// A JSON `{"error": "..."}` response. Internal details are logged, never returned.
#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    pub fn new(status: StatusCode, message: String) -> Self {
        Self { status, message }
    }

    pub fn bad_request(error: anyhow::Error) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: format!("{error:#}"),
        }
    }

    pub fn unauthorized() -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            message: "invalid administrator token".into(),
        }
    }

    pub fn not_found() -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: "not found".into(),
        }
    }

    pub fn internal(error: anyhow::Error) -> Self {
        tracing::error!(error = format!("{error:#}"), "management operation failed");
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: "internal error; see the service log".into(),
        }
    }
}

impl From<UpdateError> for ApiError {
    fn from(error: UpdateError) -> Self {
        match error {
            UpdateError::NotFound => Self::not_found(),
            UpdateError::Invalid(error) => Self::bad_request(error),
            UpdateError::Storage(error) => Self::internal(error),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "error": self.message }))).into_response()
    }
}
