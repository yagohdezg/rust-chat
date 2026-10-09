use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

use chat_computers::ComputerError;
use chat_core::ChatError;

/// Wrapper so `ChatError` can be returned directly from axum handlers.
pub struct ApiError(pub ChatError);

impl From<ComputerError> for ApiError {
    fn from(err: ComputerError) -> Self {
        Self(ChatError::Internal(anyhow::anyhow!(err)))
    }
}

impl From<ChatError> for ApiError {
    fn from(err: ChatError) -> Self {
        Self(err)
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(err: anyhow::Error) -> Self {
        Self(ChatError::Internal(err))
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(err: sqlx::Error) -> Self {
        Self(ChatError::Database(err))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status =
            StatusCode::from_u16(self.0.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        if status.is_server_error() {
            tracing::error!(error = %self.0, "request failed");
        }
        (status, Json(json!({ "error": self.0.to_string() }))).into_response()
    }
}
