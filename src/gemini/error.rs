//! Error types for gemini.

use std::time::Duration;

/// Result type alias using [`Error`].
pub type Result<T> = std::result::Result<T, Error>;

/// Errors that can occur when using the Cloud Code API.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Authentication-related errors.
    #[error("Authentication error: {0}")]
    Auth(#[from] AuthError),

    /// API errors returned by Cloud Code.
    #[error("API error ({status}): {message}")]
    Api {
        /// HTTP status code.
        status: u16,
        /// Error message from the API.
        message: String,
        /// Retry-after duration for rate limits.
        retry_after: Option<Duration>,
    },

    /// Network/HTTP errors.
    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),

    /// JSON serialization/deserialization errors.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// Configuration errors.
    #[error("Configuration error: {0}")]
    Config(String),

    /// Token storage errors.
    #[error("Storage error: {0}")]
    Storage(String),

    /// I/O errors.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

impl Error {
    /// Create a configuration error.
    pub fn config(msg: impl Into<String>) -> Self {
        Error::Config(msg.into())
    }

    /// Create a storage error.
    pub fn storage(msg: impl Into<String>) -> Self {
        Error::Storage(msg.into())
    }

    /// Create an API error.
    pub fn api(status: u16, message: impl Into<String>, retry_after: Option<Duration>) -> Self {
        Error::Api {
            status,
            message: message.into(),
            retry_after,
        }
    }

    /// Check if this is a rate limit error.
    pub fn is_rate_limit(&self) -> bool {
        matches!(self, Error::Api { status: 429, .. })
    }

    /// Check if this is an authentication error.
    pub fn is_auth_error(&self) -> bool {
        matches!(self, Error::Auth(_) | Error::Api { status: 401, .. })
    }

    /// Get retry-after duration if this is a rate limit error.
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            Error::Api { retry_after, .. } => *retry_after,
            _ => None,
        }
    }
}

/// Authentication-specific errors.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    /// No valid credentials are stored.
    #[error("Not authenticated - please complete OAuth flow")]
    NotAuthenticated,

    /// Access token has expired and refresh failed.
    #[error("Token expired - please re-authenticate")]
    TokenExpired,

    /// Refresh token is invalid (revoked or corrupted).
    #[error("Invalid grant - refresh token is invalid")]
    InvalidGrant,

    /// OAuth state mismatch (potential CSRF).
    #[error("OAuth state mismatch - possible CSRF attack")]
    StateMismatch,

    /// OAuth flow was cancelled or timed out.
    #[error("OAuth flow cancelled")]
    Cancelled,

    /// Project discovery failed.
    #[error("Failed to discover project: {0}")]
    ProjectDiscovery(String),
}

#[cfg(feature = "system-keyring")]
impl From<keyring::Error> for Error {
    fn from(e: keyring::Error) -> Self {
        Error::Storage(format!("Keyring error: {}", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = Error::config("missing required field");
        assert_eq!(
            err.to_string(),
            "Configuration error: missing required field"
        );

        let err = Error::api(429, "rate limited", Some(Duration::from_secs(60)));
        assert!(err.to_string().contains("429"));
        assert!(err.is_rate_limit());
        assert_eq!(err.retry_after(), Some(Duration::from_secs(60)));
    }

    #[test]
    fn test_auth_error() {
        let err = Error::Auth(AuthError::NotAuthenticated);
        assert!(err.is_auth_error());
        assert!(!err.is_rate_limit());
    }
}
