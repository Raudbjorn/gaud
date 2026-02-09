//! File-based token storage with secure permissions.

use async_trait::async_trait;
use std::collections::HashMap;
use std::path::PathBuf;
use tracing::debug;

use super::TokenStorage;
use crate::error::{Error, Result};
use crate::models::auth::KiroTokenInfo;

/// File-based token storage using JSON with 0600 permissions.
pub struct FileTokenStorage {
    path: PathBuf,
}

impl FileTokenStorage {
    /// Create storage at the specified path.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Create storage at the default path: `~/.config/kiro-gateway/tokens.json`
    pub fn default_path() -> Result<Self> {
        let config_dir = dirs::config_dir()
            .ok_or_else(|| Error::Config("Cannot determine config directory".into()))?;
        let path = config_dir.join("kiro-gateway").join("tokens.json");
        Ok(Self::new(path))
    }

    fn read_all(&self) -> Result<HashMap<String, KiroTokenInfo>> {
        if !self.path.exists() {
            return Ok(HashMap::new());
        }
        let content = std::fs::read_to_string(&self.path)
            .map_err(|e| Error::storage_io(&self.path, e.to_string()))?;
        if content.trim().is_empty() {
            return Ok(HashMap::new());
        }
        serde_json::from_str(&content).map_err(|e| Error::StorageSerialization(e.to_string()))
    }

    fn write_all(&self, data: &HashMap<String, KiroTokenInfo>) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| Error::storage_io(parent, e.to_string()))?;
        }

        let content =
            serde_json::to_string_pretty(data).map_err(|e| Error::StorageSerialization(e.to_string()))?;
        std::fs::write(&self.path, &content)
            .map_err(|e| Error::storage_io(&self.path, e.to_string()))?;

        // Set 0600 permissions on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(&self.path, perms)
                .map_err(|e| Error::storage_io(&self.path, format!("chmod: {}", e)))?;
        }

        debug!(path = %self.path.display(), "Token saved");
        Ok(())
    }
}

#[async_trait]
impl TokenStorage for FileTokenStorage {
    async fn load(&self, provider: &str) -> Result<Option<KiroTokenInfo>> {
        let data = self.read_all()?;
        Ok(data.get(provider).cloned())
    }

    async fn save(&self, provider: &str, token: &KiroTokenInfo) -> Result<()> {
        let mut data = self.read_all()?;
        data.insert(provider.to_string(), token.clone());
        self.write_all(&data)
    }

    async fn remove(&self, provider: &str) -> Result<()> {
        let mut data = self.read_all()?;
        data.remove(provider);
        self.write_all(&data)
    }

    fn name(&self) -> &str {
        "file"
    }
}
