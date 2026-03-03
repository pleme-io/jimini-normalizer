use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct FieldError {
    pub field: String,
    pub message: String,
}

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("validation failed: {0:?}")]
    Validation(Vec<FieldError>),

    #[error("unknown provider: {0}")]
    UnknownProvider(String),

    #[error("parse error: {0}")]
    ParseError(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("internal error: {0}")]
    Internal(String),
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
    code: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<Vec<FieldError>>,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, error_msg, details) = match &self {
            AppError::Validation(fields) => (
                StatusCode::BAD_REQUEST,
                "Validation failed".to_string(),
                Some(fields.clone()),
            ),
            AppError::UnknownProvider(name) => (
                StatusCode::BAD_REQUEST,
                format!("Unknown provider: {name}"),
                None,
            ),
            AppError::ParseError(msg) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("Failed to parse input: {msg}"),
                None,
            ),
            AppError::NotFound(msg) => (StatusCode::NOT_FOUND, msg.clone(), None),
            AppError::Internal(msg) => {
                tracing::error!(error = %msg, "internal server error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Internal server error".to_string(),
                    None,
                )
            }
        };

        let body = ErrorResponse {
            error: error_msg,
            code: status.as_u16(),
            details,
        };

        (status, axum::Json(body)).into_response()
    }
}

impl Clone for FieldError {
    fn clone(&self) -> Self {
        Self {
            field: self.field.clone(),
            message: self.message.clone(),
        }
    }
}
