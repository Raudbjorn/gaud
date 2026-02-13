//! Token storage trait.

use crate::auth::error::AuthError;
use crate::auth::tokens::TokenInfo;
use std::sync::Arc;

/// Trait for token storage backends.
///
/// All storage implementations must be thread-safe (`Send + Sync`).
/// Operations take a `provider` parameter (e.g., "claude", "gemini") to
/// support storing tokens for multiple LLM providers.
pub trait TokenStorage: Send + Sync {
    /// Load the stored token for a provider, if any.
    fn load(&self, provider: &str) -> Result<Option<TokenInfo>, AuthError>;

    /// Save a token for a provider to storage.
    fn save(&self, provider: &str, token: &TokenInfo) -> Result<(), AuthError>;

    /// Remove the stored token for a provider.
    fn remove(&self, provider: &str) -> Result<(), AuthError>;

    /// Check if a token exists in storage for a provider.
    fn exists(&self, provider: &str) -> Result<bool, AuthError> {
        Ok(self.load(provider)?.is_some())
    }

    /// Get the name of this storage backend.
    fn name(&self) -> &str;
}

// Blanket implementation for Arc<T>
impl<T: TokenStorage + ?Sized> TokenStorage for Arc<T> {
    fn load(&self, provider: &str) -> Result<Option<TokenInfo>, AuthError> {
        (**self).load(provider)
    }
    fn save(&self, provider: &str, token: &TokenInfo) -> Result<(), AuthError> {
        (**self).save(provider, token)
    }
    fn remove(&self, provider: &str) -> Result<(), AuthError> {
        (**self).remove(provider)
    }
    fn exists(&self, provider: &str) -> Result<bool, AuthError> {
        (**self).exists(provider)
    }
    fn name(&self) -> &str {
        (**self).name()
    }
}

// Blanket implementation for Box<T>
impl<T: TokenStorage + ?Sized> TokenStorage for Box<T> {
    fn load(&self, provider: &str) -> Result<Option<TokenInfo>, AuthError> {
        (**self).load(provider)
    }
    fn save(&self, provider: &str, token: &TokenInfo) -> Result<(), AuthError> {
        (**self).save(provider, token)
    }
    fn remove(&self, provider: &str) -> Result<(), AuthError> {
        (**self).remove(provider)
    }
    fn exists(&self, provider: &str) -> Result<bool, AuthError> {
        (**self).exists(provider)
    }
    fn name(&self) -> &str {
        (**self).name()
    }
}
