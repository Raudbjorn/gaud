//! Token storage backends for persisting Kiro credentials.
//!
//! Provides the [`TokenStorage`] trait and implementations:
//! - [`FileTokenStorage`] - JSON file with 0600 permissions
//! - [`MemoryTokenStorage`] - In-memory (testing)
//! - [`CallbackStorage`] - User-provided callbacks
//! - [`KeyringTokenStorage`] - System keyring (feature-gated)

mod callback;
mod file;
mod memory;

#[cfg(feature = "keyring")]
mod keyring;

use async_trait::async_trait;

pub use callback::CallbackStorage;
pub use file::FileTokenStorage;
pub use memory::MemoryTokenStorage;

#[cfg(feature = "keyring")]
pub use keyring::KeyringTokenStorage;

use crate::error::Result;
use crate::models::auth::KiroTokenInfo;

/// Trait for token storage backends.
///
/// All operations take a `provider` parameter to support multiple
/// providers in a single storage backend. For Kiro, use `"kiro"`.
#[async_trait]
pub trait TokenStorage: Send + Sync {
    /// Load stored token for a provider.
    async fn load(&self, provider: &str) -> Result<Option<KiroTokenInfo>>;

    /// Save token for a provider.
    async fn save(&self, provider: &str, token: &KiroTokenInfo) -> Result<()>;

    /// Remove stored token for a provider.
    async fn remove(&self, provider: &str) -> Result<()>;

    /// Check if a token exists for a provider.
    async fn exists(&self, provider: &str) -> Result<bool> {
        Ok(self.load(provider).await?.is_some())
    }

    /// Name of this storage backend.
    fn name(&self) -> &str {
        "unknown"
    }
}

/// Blanket impl for `Arc<T>`.
#[async_trait]
impl<T: TokenStorage + ?Sized> TokenStorage for std::sync::Arc<T> {
    async fn load(&self, provider: &str) -> Result<Option<KiroTokenInfo>> {
        (**self).load(provider).await
    }
    async fn save(&self, provider: &str, token: &KiroTokenInfo) -> Result<()> {
        (**self).save(provider, token).await
    }
    async fn remove(&self, provider: &str) -> Result<()> {
        (**self).remove(provider).await
    }
    async fn exists(&self, provider: &str) -> Result<bool> {
        (**self).exists(provider).await
    }
    fn name(&self) -> &str {
        (**self).name()
    }
}

/// Blanket impl for `Box<T>`.
#[async_trait]
impl<T: TokenStorage + ?Sized> TokenStorage for Box<T> {
    async fn load(&self, provider: &str) -> Result<Option<KiroTokenInfo>> {
        (**self).load(provider).await
    }
    async fn save(&self, provider: &str, token: &KiroTokenInfo) -> Result<()> {
        (**self).save(provider, token).await
    }
    async fn remove(&self, provider: &str) -> Result<()> {
        (**self).remove(provider).await
    }
    async fn exists(&self, provider: &str) -> Result<bool> {
        (**self).exists(provider).await
    }
    fn name(&self) -> &str {
        (**self).name()
    }
}
