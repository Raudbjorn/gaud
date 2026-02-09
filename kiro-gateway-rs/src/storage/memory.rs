//! In-memory token storage for testing.

use async_trait::async_trait;
use std::collections::HashMap;
use tokio::sync::RwLock;

use super::TokenStorage;
use crate::error::Result;
use crate::models::auth::KiroTokenInfo;

/// In-memory token storage, primarily for testing.
pub struct MemoryTokenStorage {
    tokens: RwLock<HashMap<String, KiroTokenInfo>>,
}

impl MemoryTokenStorage {
    /// Create a new empty in-memory storage.
    pub fn new() -> Self {
        Self {
            tokens: RwLock::new(HashMap::new()),
        }
    }
}

impl Default for MemoryTokenStorage {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl TokenStorage for MemoryTokenStorage {
    async fn load(&self, provider: &str) -> Result<Option<KiroTokenInfo>> {
        Ok(self.tokens.read().await.get(provider).cloned())
    }

    async fn save(&self, provider: &str, token: &KiroTokenInfo) -> Result<()> {
        self.tokens
            .write()
            .await
            .insert(provider.to_string(), token.clone());
        Ok(())
    }

    async fn remove(&self, provider: &str) -> Result<()> {
        self.tokens.write().await.remove(provider);
        Ok(())
    }

    async fn exists(&self, provider: &str) -> Result<bool> {
        Ok(self.tokens.read().await.contains_key(provider))
    }

    fn name(&self) -> &str {
        "memory"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_memory_storage() {
        let storage = MemoryTokenStorage::new();

        assert!(storage.load("kiro").await.unwrap().is_none());
        assert!(!storage.exists("kiro").await.unwrap());

        let token = KiroTokenInfo::new("refresh_token".into());
        storage.save("kiro", &token).await.unwrap();

        assert!(storage.exists("kiro").await.unwrap());
        let loaded = storage.load("kiro").await.unwrap().unwrap();
        assert_eq!(loaded.refresh_token, "refresh_token");

        storage.remove("kiro").await.unwrap();
        assert!(!storage.exists("kiro").await.unwrap());
    }
}
