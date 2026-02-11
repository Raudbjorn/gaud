//! Error types for kiro-gateway.

use std::path::PathBuf;
use std::time::Duration;
use thiserror::Error;

/// The main error type for kiro-gateway.
#[derive(Debug, Error)]
pub enum Error {
    // ── Authentication ───────────────────────────────────────────────────────
    /// No credentials available - provide a refresh token or credentials source.
    #[error("Not authenticated - provide refresh token, credentials file, or SQLite DB path")]
    NotAuthenticated,

    /// Token has expired and refresh failed.
    #[error("Token expired")]
    TokenExpired,

    /// Token refresh failed.
    #[error("Token refresh failed: {0}")]
    RefreshFailed(String),

    /// Missing required credential field.
    #[error("Missing credential: {0}")]
    MissingCredential(String),

    // ── API ──────────────────────────────────────────────────────────────────
    /// API returned an error response.
    #[error("API error {status}: {message}")]
    Api {
        /// HTTP status code.
        status: u16,
        /// Error message from the API.
        message: String,
    },

    /// Rate limited by the API.
    #[error("Rate limited - retry after {retry_after:?}")]
    RateLimited {
        /// Suggested retry delay, if provided.
        retry_after: Option<Duration>,
    },

    /// All retry attempts exhausted.
    #[error("Request failed after {attempts} attempts: {message}")]
    RetriesExhausted {
        /// Number of attempts made.
        attempts: u32,
        /// Description of the last error.
        message: String,
    },

    // ── Conversion ───────────────────────────────────────────────────────────
    /// Error converting between Anthropic and Kiro formats.
    #[error("Conversion error: {0}")]
    Conversion(String),

    /// No messages provided in request.
    #[error("No messages to send")]
    EmptyMessages,

    // ── Storage ──────────────────────────────────────────────────────────────
    /// Storage I/O error.
    #[error("Storage I/O error at {path}: {message}")]
    StorageIo {
        /// Path that caused the error.
        path: PathBuf,
        /// Error description.
        message: String,
    },

    /// Storage serialization error.
    #[error("Storage serialization error: {0}")]
    StorageSerialization(String),

    /// Keyring backend error.
    #[error("Keyring error: {0}")]
    Keyring(String),

    /// Generic storage error.
    #[error("Storage error: {0}")]
    Storage(String),

    // ── Infrastructure ───────────────────────────────────────────────────────
    /// Network/HTTP error.
    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),

    /// JSON parsing error.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// General I/O error.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Streaming error.
    #[error("Stream error: {0}")]
    Stream(String),

    /// Configuration error.
    #[error("Configuration error: {0}")]
    Config(String),

    /// Request timeout.
    #[error("Request timed out")]
    Timeout,
}

impl Error {
    /// Returns true if this error indicates re-authentication is needed.
    #[must_use]
    pub fn requires_reauth(&self) -> bool {
        matches!(
            self,
            Error::NotAuthenticated
                | Error::TokenExpired
                | Error::RefreshFailed(_)
                | Error::Api { status: 401, .. }
                | Error::Api { status: 403, .. }
        )
    }

    /// Creates a storage I/O error.
    #[must_use]
    pub fn storage_io(path: impl Into<PathBuf>, message: impl Into<String>) -> Self {
        Self::StorageIo {
            path: path.into(),
            message: message.into(),
        }
    }
}

/// Convenience type alias.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_requires_reauth() {
        assert!(Error::NotAuthenticated.requires_reauth());
        assert!(Error::TokenExpired.requires_reauth());
        assert!(Error::RefreshFailed("test".into()).requires_reauth());
        assert!(Error::Api { status: 403, message: "Forbidden".into() }.requires_reauth());

        assert!(!Error::Api { status: 500, message: "Server error".into() }.requires_reauth());
        assert!(!Error::Timeout.requires_reauth());
    }

    #[test]
    fn test_error_display() {
        let err = Error::NotAuthenticated;
        assert!(err.to_string().contains("Not authenticated"));

        let err = Error::Api { status: 429, message: "Too many requests".into() };
        assert_eq!(err.to_string(), "API error 429: Too many requests");
    }
}
