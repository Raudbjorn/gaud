//! Shared authentication traits.

use crate::auth::error::AuthError;

/// Trait for providing access tokens.
///
/// This abstracts the source of tokens (e.g., OAuthManager, static token)
/// from the consumers (e.g., API clients).
#[async_trait::async_trait]
pub trait TokenProvider: Send + Sync {
    /// Get a valid access token for the specified provider.
    ///
    /// The implementation should handle refreshing if necessary.
    async fn get_token(&self, provider: &str) -> Result<String, AuthError>;
}

/// Trait for authenticating HTTP requests.
///
/// Implementations can add headers (e.g. Authorization) or query parameters
/// to authenticate requests.
#[async_trait::async_trait]
pub trait AuthProvider: Send + Sync {
    /// Authenticate the request builder.
    async fn authenticate(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<reqwest::RequestBuilder, AuthError>;
}
