use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use crate::providers::ProviderError;

/// Unified application error type following OpenAI error format.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("Authentication required: {0}")]
    Unauthorized(String),

    #[error("Forbidden: {0}")]
    Forbidden(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Bad request: {0}")]
    BadRequest(String),

    #[error("Budget exceeded: {0}")]
    BudgetExceeded(String),

    #[error("Rate limited: {0}")]
    RateLimited(String),

    #[error("Context window exceeded: {0}")]
    ContextWindow(String),

    #[error("Provider error ({status}): {message}")]
    ProviderWithStatus { status: u16, message: String },

    #[error("Provider error: {0}")]
    Provider(String),

    #[error("OAuth error: {0}")]
    OAuth(String),

    #[error("Database error: {0}")]
    Database(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

/// OpenAI-compatible error response body.
#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: ErrorDetail,
}

#[derive(Debug, Serialize)]
struct ErrorDetail {
    message: String,
    r#type: String,
    code: Option<String>,
}

impl AppError {
    fn status_code(&self) -> StatusCode {
        match self {
            Self::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            Self::Forbidden(_) => StatusCode::FORBIDDEN,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::BadRequest(_) | Self::ContextWindow(_) => StatusCode::BAD_REQUEST,
            Self::BudgetExceeded(_) | Self::RateLimited(_) => StatusCode::TOO_MANY_REQUESTS,
            Self::ProviderWithStatus { status, .. } => {
                StatusCode::from_u16(*status).unwrap_or(StatusCode::BAD_GATEWAY)
            }
            Self::Provider(_) => StatusCode::BAD_GATEWAY,
            Self::OAuth(_) => StatusCode::BAD_REQUEST,
            Self::Database(_) | Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn error_type(&self) -> &str {
        match self {
            Self::Unauthorized(_) => "authentication_error",
            Self::Forbidden(_) => "permission_error",
            Self::NotFound(_) => "not_found_error",
            Self::BadRequest(_) | Self::ContextWindow(_) => "invalid_request_error",
            Self::BudgetExceeded(_) => "rate_limit_error",
            Self::RateLimited(_) => "rate_limit_error",
            Self::Provider(_) | Self::ProviderWithStatus { .. } => "api_error",
            Self::OAuth(_) => "oauth_error",
            Self::Database(_) | Self::Internal(_) => "server_error",
        }
    }

    fn error_code(&self) -> Option<&str> {
        match self {
            Self::BudgetExceeded(_) => Some("budget_exceeded"),
            Self::RateLimited(_) => Some("rate_limit_exceeded"),
            Self::ContextWindow(_) => Some("context_length_exceeded"),
            Self::Unauthorized(_) => Some("invalid_api_key"),
            _ => None,
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.status_code();
        let body = ErrorResponse {
            error: ErrorDetail {
                message: self.to_string(),
                r#type: self.error_type().to_string(),
                code: self.error_code().map(String::from),
            },
        };
        (status, axum::Json(body)).into_response()
    }
}

impl From<rusqlite::Error> for AppError {
    fn from(err: rusqlite::Error) -> Self {
        tracing::error!(error = %err, "Database error");
        Self::Database(err.to_string())
    }
}

impl From<reqwest::Error> for AppError {
    fn from(err: reqwest::Error) -> Self {
        tracing::error!(error = %err, "HTTP client error");
        Self::Provider(err.to_string())
    }
}

impl From<serde_json::Error> for AppError {
    fn from(err: serde_json::Error) -> Self {
        Self::BadRequest(format!("JSON error: {err}"))
    }
}

impl From<anyhow::Error> for AppError {
    fn from(err: anyhow::Error) -> Self {
        Self::Internal(err.to_string())
    }
}

impl From<ProviderError> for AppError {
    fn from(err: ProviderError) -> Self {
        match &err {
            // Preserve 429 status for rate limiting.
            ProviderError::RateLimited { .. } => Self::RateLimited(err.to_string()),

            // Context window errors -> 400.
            ProviderError::ContextWindowExceeded { .. } => Self::ContextWindow(err.to_string()),

            // Auth errors -> 401.
            ProviderError::Authentication { .. } | ProviderError::NoToken { .. } => {
                Self::Unauthorized(err.to_string())
            }

            // Invalid request -> 400.
            ProviderError::InvalidRequest(_) => Self::BadRequest(err.to_string()),

            // API errors preserve upstream status code.
            ProviderError::Api { status, message } => Self::ProviderWithStatus {
                status: *status,
                message: message.clone(),
            },

            // Everything else -> 502 (Bad Gateway).
            _ => Self::Provider(err.to_string()),
        }
    }
}
