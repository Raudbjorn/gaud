//! Error types for auth module.

use std::error::Error as StdError;


/// Errors that can occur during Auth operations.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    /// Token not found for the given provider.
    #[error("Token not found for {0}")]
    TokenNotFound(String),

    /// Token has expired and needs re-authentication.
    #[error("Token expired for {0}")]
    TokenExpired(String),

    /// Token exchange or refresh failed.
    #[error("Exchange failed: {0}")]
    ExchangeFailed(String),

    /// Token storage error.
    #[error("Storage error: {0}")]
    Storage(String),

    /// Invalid or unknown state token (possible CSRF).
    #[error("Invalid state token")]
    InvalidState,

    /// The OAuth flow has expired (e.g., device code timeout).
    #[error("Flow expired")]
    FlowExpired,

    /// HTTP client error.
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    /// Redis authentication error.
    #[error("Redis auth error: {0}")]
    Redis(String),

    /// Generic error.
    #[error("{0}")]
    Other(String),
}

// Implement From for any error type that can be converted to String
impl From<Box<dyn StdError + Send + Sync>> for AuthError {
    fn from(err: Box<dyn StdError + Send + Sync>) -> Self {
        AuthError::Other(err.to_string())
    }
}
