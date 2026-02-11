//! Token storage backends for persisting OAuth credentials.
//!
//! This module provides the [`TokenStorage`] trait and several implementations:
//!
//! - [`FileTokenStorage`] - Stores tokens in a JSON file with secure permissions
//! - [`MemoryTokenStorage`] - In-memory storage for testing
//! - [`CallbackStorage`] - Custom storage via callbacks
//! - [`KeyringTokenStorage`] - System keyring storage (requires `keyring` feature)
//!
//! # Security
//!
//! - File storage uses 0600 permissions on Unix systems
//! - Tokens are never logged (use `#[instrument(skip(token))]`)
//! - All implementations are thread-safe (`Send + Sync`)
//!
//! # Example
//!
//! ```rust,ignore
//! use antigravity_gate::storage::{TokenStorage, FileTokenStorage};
//!
//! # async fn example() -> antigravity_gate::Result<()> {
//! // Create storage with default path
//! let storage = FileTokenStorage::default_path()?;
//!
//! // Check if token exists
//! if storage.exists().await? {
//!     let token = storage.load().await?.unwrap();
//!     println!("Token expires at: {}", token.expires_at);
//! }
//! # Ok(())
//! # }
//! ```

mod callback;
mod file;
mod memory;

#[cfg(feature = "keyring")]
mod keyring;

use async_trait::async_trait;

pub use callback::{CallbackStorage, EnvSource, FileSource};
pub use file::FileTokenStorage;
pub use memory::MemoryTokenStorage;

#[cfg(feature = "keyring")]
pub use keyring::KeyringTokenStorage;

use crate::auth::TokenInfo;
use crate::Result;

/// Trait for token storage backends.
///
/// All storage implementations must be thread-safe (`Send + Sync`)
/// to support concurrent access from multiple tasks.
///
/// # Security Notes
///
/// - Never log token values in implementations
/// - Use `#[instrument(skip(token))]` when tracing save operations
/// - Ensure file permissions are restrictive (0600 on Unix)
///
/// # Example Implementation
///
/// ```rust,ignore
/// use async_trait::async_trait;
/// use antigravity_gate::{TokenStorage, TokenInfo, Result};
///
/// struct MyStorage { /* ... */ }
///
/// #[async_trait]
/// impl TokenStorage for MyStorage {
///     async fn load(&self) -> Result<Option<TokenInfo>> {
///         // Load token from storage
///         todo!()
///     }
///
///     async fn save(&self, token: &TokenInfo) -> Result<()> {
///         // Save token to storage
///         todo!()
///     }
///
///     async fn remove(&self) -> Result<()> {
///         // Remove token from storage
///         todo!()
///     }
///
///     fn name(&self) -> &str {
///         "my-storage"
///     }
/// }
/// ```
#[async_trait]
pub trait TokenStorage: Send + Sync {
    /// Load the stored token, if any.
    ///
    /// Returns `Ok(None)` if no token is stored.
    /// Returns `Err` if there's an error accessing storage.
    async fn load(&self) -> Result<Option<TokenInfo>>;

    /// Save a token to storage.
    ///
    /// Overwrites any existing token. Implementations should ensure
    /// appropriate file permissions and atomic writes.
    async fn save(&self, token: &TokenInfo) -> Result<()>;

    /// Remove the stored token.
    ///
    /// Returns `Ok(())` even if no token was stored.
    async fn remove(&self) -> Result<()>;

    /// Check if a token exists in storage.
    ///
    /// Default implementation calls `load()`, but implementations
    /// may provide a more efficient check.
    async fn exists(&self) -> Result<bool> {
        Ok(self.load().await?.is_some())
    }

    /// Get the name of this storage backend.
    ///
    /// Used for logging and debugging. Default is "unknown".
    fn name(&self) -> &str {
        "unknown"
    }
}

/// Blanket implementation for `Arc<T>` where T: TokenStorage
#[async_trait]
impl<T: TokenStorage + ?Sized> TokenStorage for std::sync::Arc<T> {
    async fn load(&self) -> Result<Option<TokenInfo>> {
        (**self).load().await
    }

    async fn save(&self, token: &TokenInfo) -> Result<()> {
        (**self).save(token).await
    }

    async fn remove(&self) -> Result<()> {
        (**self).remove().await
    }

    async fn exists(&self) -> Result<bool> {
        (**self).exists().await
    }

    fn name(&self) -> &str {
        (**self).name()
    }
}

/// Blanket implementation for `Box<T>` where T: TokenStorage
#[async_trait]
impl<T: TokenStorage + ?Sized> TokenStorage for Box<T> {
    async fn load(&self) -> Result<Option<TokenInfo>> {
        (**self).load().await
    }

    async fn save(&self, token: &TokenInfo) -> Result<()> {
        (**self).save(token).await
    }

    async fn remove(&self) -> Result<()> {
        (**self).remove().await
    }

    async fn exists(&self) -> Result<bool> {
        (**self).exists().await
    }

    fn name(&self) -> &str {
        (**self).name()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    // Test that Arc<T> implements TokenStorage when T does
    #[tokio::test]
    async fn test_arc_storage() {
        let storage = Arc::new(MemoryTokenStorage::new());

        // Create a token
        let token = TokenInfo::new("access".into(), "refresh".into(), 3600);

        // Save via Arc
        storage.save(&token).await.unwrap();

        // Load via Arc
        let loaded = storage.load().await.unwrap().unwrap();
        assert_eq!(loaded.access_token, "access");

        // Check name
        assert_eq!(storage.name(), "memory");
    }

    // Test that Box<dyn TokenStorage> works
    #[tokio::test]
    async fn test_box_dyn_storage() {
        let storage: Box<dyn TokenStorage> = Box::new(MemoryTokenStorage::new());

        let token = TokenInfo::new("access".into(), "refresh".into(), 3600);
        storage.save(&token).await.unwrap();

        let loaded = storage.load().await.unwrap().unwrap();
        assert_eq!(loaded.access_token, "access");
    }
}
