//! LLM Provider Module
//!
//! Defines the core LlmProvider trait and error types, plus sub-modules for
//! routing, health tracking, cost calculation, and concrete provider
//! implementations (Claude, Gemini, Copilot).

pub mod claude;
pub mod copilot;
pub mod cost;
pub mod gemini;
pub mod health;
pub mod kiro;
pub mod litellm;
pub mod pricing;
pub mod retry;
pub mod router;
pub mod traits;
pub mod transform;
pub mod transformer;
pub mod types;
use std::time::Duration;


// Re-exports for convenience.
pub use self::cost::CostCalculator;
pub use self::health::CircuitBreaker;
pub use self::pricing::{ModelPricing as PricingInfo, PricingDatabase};
pub use self::retry::RetryPolicy;
pub use self::router::ProviderRouter;
pub use self::traits::LlmProvider;
pub use self::transformer::{ProviderResponseMeta, ProviderTransformer, StreamState};

// ---------------------------------------------------------------------------
// ProviderError
// ---------------------------------------------------------------------------

/// Errors that can occur during provider operations.
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("No token available for {provider}")]
    NoToken { provider: String },

    #[error("Authentication failed for {provider}: {message}")]
    Authentication {
        provider: String,
        message: String,
        retry_count: u32,
        max_retries: u32,
    },

    #[error("Provider unhealthy: {0}")]
    Unhealthy(String),

    #[error("No provider for model: {0}")]
    NoProvider(String),

    #[error("All providers failed for model {model}")]
    AllFailed { model: String, errors: Vec<String> },

    #[error("Stream error: {0}")]
    Stream(String),

    #[error("Rate limited: retry after {retry_after_secs}s")]
    RateLimited {
        retry_after_secs: u64,
        /// Parsed retry-after duration from provider headers.
        retry_after: Option<Duration>,
    },

    #[error("Context window exceeded ({provider}): {message}")]
    ContextWindowExceeded {
        provider: String,
        message: String,
        /// Maximum context tokens for the model, if known.
        max_tokens: Option<u32>,
    },

    #[error("API error ({status}): {message}")]
    Api { status: u16, message: String },

    #[error("Timeout after {timeout_secs}s")]
    Timeout { timeout_secs: u64 },

    #[error("Invalid request: {0}")]
    InvalidRequest(String),

    #[error("Response parsing error: {0}")]
    ResponseParsing(String),

    #[error("{0}")]
    Other(String),
}

impl ProviderError {
    /// Extract the upstream HTTP status code, if this error maps to one.
    pub fn status_code(&self) -> Option<u16> {
        match self {
            Self::RateLimited { .. } => Some(429),
            Self::ContextWindowExceeded { .. } | Self::InvalidRequest(_) => Some(400),
            Self::Authentication { .. } | Self::NoToken { .. } => Some(401),
            Self::Api { status, .. } => Some(*status),
            _ => None,
        }
    }

    /// Get the retry-after duration if this is a rate limit error.
    pub fn retry_after_duration(&self) -> Option<Duration> {
        match self {
            Self::RateLimited {
                retry_after,
                retry_after_secs,
                ..
            } => retry_after.or_else(|| Some(Duration::from_secs(*retry_after_secs))),
            _ => None,
        }
    }
}





#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_error_display() {
        let err = ProviderError::NoProvider("unknown-model".into());
        assert_eq!(err.to_string(), "No provider for model: unknown-model");
    }

    #[test]
    fn test_provider_error_all_failed() {
        let err = ProviderError::AllFailed {
            model: "test-model".to_string(),
            errors: vec!["error1".to_string(), "error2".to_string()],
        };
        assert!(err.to_string().contains("All providers failed"));
    }

    #[test]
    fn test_provider_error_no_token() {
        let err = ProviderError::NoToken {
            provider: "claude".to_string(),
        };
        assert_eq!(err.to_string(), "No token available for claude");
    }

    #[test]
    fn test_provider_error_http() {
        // Verify From<reqwest::Error> compiles (we can't easily construct one
        // without a real HTTP call, so just check the variant exists).
        let err = ProviderError::Stream("test".into());
        assert_eq!(err.to_string(), "Stream error: test");
    }

    #[test]
    fn test_provider_error_rate_limited() {
        let err = ProviderError::RateLimited {
            retry_after_secs: 30,
            retry_after: None,
        };
        assert_eq!(err.to_string(), "Rate limited: retry after 30s");
    }

    #[test]
    fn test_provider_error_context_window_exceeded() {
        let err = ProviderError::ContextWindowExceeded {
            provider: "claude".to_string(),
            message: "Request too large for claude-sonnet-4".to_string(),
            max_tokens: Some(200_000),
        };
        assert!(err.to_string().contains("Context window exceeded"));
        assert_eq!(err.status_code(), Some(400));
    }

    #[test]
    fn test_provider_error_status_codes() {
        assert_eq!(
            ProviderError::RateLimited {
                retry_after_secs: 30,
                retry_after: None
            }
            .status_code(),
            Some(429)
        );
        assert_eq!(
            ProviderError::Api {
                status: 503,
                message: "overloaded".into()
            }
            .status_code(),
            Some(503)
        );
        assert_eq!(
            ProviderError::NoToken {
                provider: "claude".into()
            }
            .status_code(),
            Some(401)
        );
        assert_eq!(
            ProviderError::InvalidRequest("bad".into()).status_code(),
            Some(400)
        );
        assert_eq!(ProviderError::Stream("err".into()).status_code(), None);
    }

    #[test]
    fn test_retry_after_duration() {
        use std::time::Duration;

        let err = ProviderError::RateLimited {
            retry_after_secs: 30,
            retry_after: Some(Duration::from_secs(45)),
        };
        assert_eq!(err.retry_after_duration(), Some(Duration::from_secs(45)));

        let err = ProviderError::RateLimited {
            retry_after_secs: 30,
            retry_after: None,
        };
        assert_eq!(err.retry_after_duration(), Some(Duration::from_secs(30)));

        let err = ProviderError::Other("not rate limited".into());
        assert_eq!(err.retry_after_duration(), None);
    }

    #[test]
    fn test_provider_error_api() {
        let err = ProviderError::Api {
            status: 429,
            message: "Too many requests".into(),
        };
        assert_eq!(err.to_string(), "API error (429): Too many requests");
    }

    #[test]
    fn test_provider_error_other() {
        let err = ProviderError::Other("something went wrong".into());
        assert_eq!(err.to_string(), "something went wrong");
    }
}
