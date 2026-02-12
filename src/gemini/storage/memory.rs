//! In-memory token storage for testing and ephemeral use.
//!
//! This module provides [`MemoryTokenStorage`], a thread-safe in-memory
//! token storage backend. Useful for:
//!
//! - Unit tests that need isolated token storage
//! - Short-lived applications that don't need persistence
//! - Caching layer in front of persistent storage

use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::instrument;

use super::TokenStorage;
use crate::auth::gemini::TokenInfo;
use crate::gemini::error::Result;

/// In-memory token storage.
///
/// Uses `Arc<RwLock<Option<TokenInfo>>>` for thread-safe access from
/// multiple async tasks. The storage is Clone and can be shared across
/// the application.
///
/// # Example
///
/// ```rust
/// use gaud::gemini::storage::MemoryTokenStorage;
/// use gaud::gemini::TokenInfo;
/// use gaud::gemini::storage::TokenStorage;
///
/// # async fn example() -> gaud::gemini::Result<()> {
/// // Create empty storage
/// let storage = MemoryTokenStorage::new();
///
/// // Or create with an initial token
/// let token = TokenInfo::new("access".into(), "refresh".into(), 3600);
/// let storage = MemoryTokenStorage::with_token(token);
///
/// // Storage can be cloned and shared
/// let storage2 = storage.clone();
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct MemoryTokenStorage {
    /// Thread-safe token storage.
    inner: Arc<RwLock<Option<TokenInfo>>>,
}

impl Default for MemoryTokenStorage {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryTokenStorage {
    /// Create a new empty MemoryTokenStorage.
    ///
    /// # Example
    ///
    /// ```rust
    /// use gaud::gemini::storage::MemoryTokenStorage;
    ///
    /// let storage = MemoryTokenStorage::new();
    /// ```
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(None)),
        }
    }

    /// Create a MemoryTokenStorage with an initial token.
    ///
    /// Useful for testing scenarios where you want to start
    /// with a pre-populated token.
    ///
    /// # Example
    ///
    /// ```rust
    /// use gaud::gemini::storage::MemoryTokenStorage;
    /// use gaud::gemini::TokenInfo;
    ///
    /// let token = TokenInfo::new(
    ///     "access_token".into(),
    ///     "refresh_token".into(),
    ///     3600,
    /// );
    /// let storage = MemoryTokenStorage::with_token(token);
    /// ```
    pub fn with_token(token: TokenInfo) -> Self {
        Self {
            inner: Arc::new(RwLock::new(Some(token))),
        }
    }

    /// Get a snapshot of the current token without async.
    ///
    /// This is a blocking operation and should only be used in
    /// synchronous contexts. For async code, use [`Self::load`].
    ///
    /// # Panics
    ///
    /// Panics if the lock is poisoned.
    pub fn get_sync(&self) -> Option<TokenInfo> {
        // Try to acquire read lock without blocking in async context
        // This is safe because we're using tokio's RwLock which doesn't poison
        futures::executor::block_on(async { self.inner.read().await.clone() })
    }
}

#[async_trait]
impl TokenStorage for MemoryTokenStorage {
    #[instrument(skip(self))]
    async fn load(&self) -> Result<Option<TokenInfo>> {
        let guard = self.inner.read().await;
        Ok(guard.clone())
    }

    #[instrument(skip(self, token))]
    async fn save(&self, token: &TokenInfo) -> Result<()> {
        let mut guard = self.inner.write().await;
        *guard = Some(token.clone());
        Ok(())
    }

    #[instrument(skip(self))]
    async fn remove(&self) -> Result<()> {
        let mut guard = self.inner.write().await;
        *guard = None;
        Ok(())
    }

    async fn exists(&self) -> Result<bool> {
        let guard = self.inner.read().await;
        Ok(guard.is_some())
    }

    fn name(&self) -> &str {
        "memory"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_new_is_empty() {
        let storage = MemoryTokenStorage::new();
        assert!(storage.load().await.unwrap().is_none());
        assert!(!storage.exists().await.unwrap());
    }

    #[tokio::test]
    async fn test_with_token() {
        let token = TokenInfo::new("access".into(), "refresh".into(), 3600);
        let storage = MemoryTokenStorage::with_token(token);

        let loaded = storage.load().await.unwrap().unwrap();
        assert_eq!(loaded.access_token, "access");
        assert!(storage.exists().await.unwrap());
    }

    #[tokio::test]
    async fn test_save_and_load() {
        let storage = MemoryTokenStorage::new();

        let token = TokenInfo::new("access".into(), "refresh".into(), 3600);
        storage.save(&token).await.unwrap();

        let loaded = storage.load().await.unwrap().unwrap();
        assert_eq!(loaded.access_token, "access");
        assert_eq!(loaded.refresh_token, "refresh");
    }

    #[tokio::test]
    async fn test_remove() {
        let token = TokenInfo::new("access".into(), "refresh".into(), 3600);
        let storage = MemoryTokenStorage::with_token(token);

        assert!(storage.exists().await.unwrap());
        storage.remove().await.unwrap();
        assert!(!storage.exists().await.unwrap());
        assert!(storage.load().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_remove_empty() {
        let storage = MemoryTokenStorage::new();
        // Should not error when removing from empty storage
        storage.remove().await.unwrap();
    }

    #[tokio::test]
    async fn test_overwrite() {
        let storage = MemoryTokenStorage::new();

        let token1 = TokenInfo::new("access1".into(), "refresh1".into(), 3600);
        storage.save(&token1).await.unwrap();

        let token2 = TokenInfo::new("access2".into(), "refresh2".into(), 7200);
        storage.save(&token2).await.unwrap();

        let loaded = storage.load().await.unwrap().unwrap();
        assert_eq!(loaded.access_token, "access2");
        assert_eq!(loaded.refresh_token, "refresh2");
    }

    #[tokio::test]
    async fn test_clone_shares_state() {
        let storage1 = MemoryTokenStorage::new();
        let storage2 = storage1.clone();

        let token = TokenInfo::new("access".into(), "refresh".into(), 3600);
        storage1.save(&token).await.unwrap();

        // Storage2 should see the token saved via storage1
        let loaded = storage2.load().await.unwrap().unwrap();
        assert_eq!(loaded.access_token, "access");
    }

    #[tokio::test]
    async fn test_concurrent_access() {
        let storage = MemoryTokenStorage::new();

        // Spawn multiple tasks that read and write concurrently
        let mut handles = vec![];

        for i in 0..10 {
            let storage = storage.clone();
            let handle = tokio::spawn(async move {
                let token = TokenInfo::new(format!("access{}", i), "refresh".into(), 3600);
                storage.save(&token).await.unwrap();
                storage.load().await.unwrap()
            });
            handles.push(handle);
        }

        // All tasks should complete without panicking
        for handle in handles {
            let result = handle.await.unwrap();
            assert!(result.is_some());
        }
    }

    #[tokio::test]
    async fn test_storage_name() {
        let storage = MemoryTokenStorage::new();
        assert_eq!(storage.name(), "memory");
    }

    #[test]
    fn test_default() {
        let storage = MemoryTokenStorage::default();
        // get_sync on empty should return None
        assert!(storage.get_sync().is_none());
    }

    #[tokio::test]
    async fn test_composite_token() {
        let storage = MemoryTokenStorage::new();

        let token = TokenInfo::new("access".into(), "refresh".into(), 3600)
            .with_project_ids("proj-123", Some("managed-456"));
        storage.save(&token).await.unwrap();

        let loaded = storage.load().await.unwrap().unwrap();
        let (base, project, managed) = loaded.parse_refresh_parts();
        assert_eq!(base, "refresh");
        assert_eq!(project.as_deref(), Some("proj-123"));
        assert_eq!(managed.as_deref(), Some("managed-456"));
    }
}
