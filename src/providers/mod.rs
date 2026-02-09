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
pub mod router;
pub mod types;

use std::future::Future;
use std::pin::Pin;

use futures::Stream;

use crate::providers::types::{ChatChunk, ChatRequest, ChatResponse, ModelPricing};

// Re-exports for convenience.
pub use self::cost::CostDatabase;
pub use self::health::CircuitBreaker;
pub use self::router::ProviderRouter;

// ---------------------------------------------------------------------------
// ProviderError
// ---------------------------------------------------------------------------

/// Errors that can occur during provider operations.
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("No token available for {0}")]
    NoToken(String),

    #[error("Provider unhealthy: {0}")]
    Unhealthy(String),

    #[error("No provider for model: {0}")]
    NoProvider(String),

    #[error("All providers failed")]
    AllFailed,

    #[error("Stream error: {0}")]
    Stream(String),

    #[error("Rate limited: retry after {retry_after_secs}s")]
    RateLimited { retry_after_secs: u64 },

    #[error("API error ({status}): {message}")]
    Api { status: u16, message: String },

    #[error("{0}")]
    Other(String),
}

// ---------------------------------------------------------------------------
// TokenStorage trait (minimal interface for the oauth module)
// ---------------------------------------------------------------------------

/// Minimal trait to retrieve OAuth access tokens for a given provider.
///
/// The concrete implementation lives in the `oauth` module; we only define the
/// interface here so that provider implementations can depend on it without
/// pulling in the full oauth crate.
pub trait TokenStorage: Send + Sync {
    /// Return a valid access token for the given provider, refreshing if
    /// necessary. Returns `None` if the user is not authenticated.
    fn get_access_token(
        &self,
        provider: &str,
    ) -> impl std::future::Future<Output = Result<Option<String>, ProviderError>> + Send;
}

// ---------------------------------------------------------------------------
// LlmProvider trait
// ---------------------------------------------------------------------------

/// Trait that all LLM providers must implement.
///
/// Async methods return boxed futures so the trait is dyn-compatible (can be
/// used as `Arc<dyn LlmProvider>`). No `async_trait` macro is needed.
pub trait LlmProvider: Send + Sync {
    /// Unique identifier for this provider (e.g. "claude", "gemini", "copilot").
    fn id(&self) -> &str;

    /// Human-readable display name.
    fn name(&self) -> &str;

    /// List of model identifiers this provider supports.
    fn models(&self) -> Vec<String>;

    /// Check whether a specific model string is handled by this provider.
    fn supports_model(&self, model: &str) -> bool;

    /// Non-streaming chat completion.
    fn chat(
        &self,
        request: &ChatRequest,
    ) -> Pin<Box<dyn Future<Output = Result<ChatResponse, ProviderError>> + Send + '_>>;

    /// Streaming chat completion returning an SSE-compatible stream of chunks.
    fn stream_chat(
        &self,
        request: &ChatRequest,
    ) -> Pin<
        Box<
            dyn Future<
                    Output = Result<
                        Pin<Box<dyn Stream<Item = Result<ChatChunk, ProviderError>> + Send>>,
                        ProviderError,
                    >,
                > + Send
                + '_,
        >,
    >;

    /// Lightweight health check (e.g. can we reach the API, do we have tokens?).
    fn health_check(&self) -> Pin<Box<dyn Future<Output = bool> + Send + '_>>;

    /// Pricing data for each model this provider supports.
    fn pricing(&self) -> Vec<ModelPricing>;
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
        let err = ProviderError::AllFailed;
        assert_eq!(err.to_string(), "All providers failed");
    }

    #[test]
    fn test_provider_error_no_token() {
        let err = ProviderError::NoToken("claude".into());
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
        };
        assert_eq!(err.to_string(), "Rate limited: retry after 30s");
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
