//! In-memory token storage.

use super::TokenStorage;
use crate::auth::error::AuthError;
use crate::auth::tokens::TokenInfo;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tracing::instrument;

/// In-memory token storage.
///
/// Uses `Arc<RwLock<HashMap>>` for thread-safe access. Useful for
/// testing and ephemeral sessions. The storage is Clone and can be
/// shared across the application.
#[derive(Debug, Clone)]
pub struct MemoryTokenStorage {
    inner: Arc<RwLock<HashMap<String, TokenInfo>>>,
}

impl Default for MemoryTokenStorage {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryTokenStorage {
    /// Create a new empty MemoryTokenStorage.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Create a MemoryTokenStorage with an initial token for a provider.
    pub fn with_token(provider: impl Into<String>, token: TokenInfo) -> Self {
        let mut map = HashMap::new();
        map.insert(provider.into(), token);
        Self {
            inner: Arc::new(RwLock::new(map)),
        }
    }

    /// Get the number of stored tokens.
    pub fn len(&self) -> usize {
        self.inner.read().expect("lock poisoned").len()
    }

    /// Check if storage is empty.
    pub fn is_empty(&self) -> bool {
        self.inner.read().expect("lock poisoned").is_empty()
    }

    /// Clear all stored tokens.
    pub fn clear(&self) {
        self.inner.write().expect("lock poisoned").clear();
    }
}

impl TokenStorage for MemoryTokenStorage {
    #[instrument(skip(self))]
    fn load(&self, provider: &str) -> Result<Option<TokenInfo>, AuthError> {
        let guard = self.inner.read().expect("lock poisoned");
        Ok(guard.get(provider).cloned())
    }

    #[instrument(skip(self, token))]
    fn save(&self, provider: &str, token: &TokenInfo) -> Result<(), AuthError> {
        let mut guard = self.inner.write().expect("lock poisoned");
        guard.insert(provider.to_string(), token.clone());
        Ok(())
    }

    #[instrument(skip(self))]
    fn remove(&self, provider: &str) -> Result<(), AuthError> {
        let mut guard = self.inner.write().expect("lock poisoned");
        guard.remove(provider);
        Ok(())
    }

    fn exists(&self, provider: &str) -> Result<bool, AuthError> {
        let guard = self.inner.read().expect("lock poisoned");
        Ok(guard.contains_key(provider))
    }

    fn name(&self) -> &str {
        "memory"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_new_is_empty() {
        let storage = MemoryTokenStorage::new();
        assert!(storage.load("claude").unwrap().is_none());
        assert!(!storage.exists("claude").unwrap());
        assert!(storage.is_empty());
    }

    #[test]
    fn test_memory_with_token() {
        let token = TokenInfo::new(
            "access".into(),
            Some("refresh".into()),
            Some(3600),
            "claude",
        );
        let storage = MemoryTokenStorage::with_token("claude", token);
        let loaded = storage.load("claude").unwrap().unwrap();
        assert_eq!(loaded.access_token, "access");
        assert!(storage.exists("claude").unwrap());
        assert!(!storage.is_empty());
    }

    #[test]
    fn test_memory_save_and_load() {
        let storage = MemoryTokenStorage::new();
        let token = TokenInfo::new(
            "access".into(),
            Some("refresh".into()),
            Some(3600),
            "claude",
        );
        storage.save("claude", &token).unwrap();
        let loaded = storage.load("claude").unwrap().unwrap();
        assert_eq!(loaded.access_token, "access");
        assert_eq!(loaded.refresh_token.as_deref(), Some("refresh"));
    }

    #[test]
    fn test_memory_remove() {
        let token = TokenInfo::new(
            "access".into(),
            Some("refresh".into()),
            Some(3600),
            "claude",
        );
        let storage = MemoryTokenStorage::with_token("claude", token);
        assert!(storage.exists("claude").unwrap());
        storage.remove("claude").unwrap();
        assert!(!storage.exists("claude").unwrap());
    }
}
