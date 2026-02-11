//! File-based token storage with secure permissions.
//!
//! Stores tokens in a JSON file at a configurable path, with:
//! - File permissions set to 0600 on Unix (owner read/write only)
//! - Parent directories created with 0700 permissions
//! - Automatic `~` expansion to home directory
//! - Atomic writes via temp file + rename

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::instrument;

use super::TokenStorage;
use crate::auth::TokenInfo;
use crate::{Error, Result};

/// Default config directory name under user's home.
const CONFIG_DIR: &str = ".config/antigravity-gate";

/// Default token file name.
const TOKEN_FILE: &str = "auth.json";

/// Key used in the JSON file for the Anthropic/Claude token.
const TOKEN_KEY: &str = "anthropic";

/// File permissions for token file (Unix only): owner read/write.
#[cfg(unix)]
const FILE_MODE: u32 = 0o600;

/// Directory permissions (Unix only): owner read/write/execute.
#[cfg(unix)]
const DIR_MODE: u32 = 0o700;

/// Token file structure for JSON storage.
///
/// Stores tokens keyed by provider name, allowing for future
/// multi-provider support while maintaining compatibility.
/// Uses Value internally to be lenient with unknown keys.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct TokenFile {
    #[serde(flatten)]
    tokens: HashMap<String, serde_json::Value>,
}

impl TokenFile {
    /// Get a token by key, attempting to deserialize it.
    fn get_token(&self, key: &str) -> Result<Option<TokenInfo>> {
        match self.tokens.get(key) {
            Some(value) => {
                let token: TokenInfo = serde_json::from_value(value.clone()).map_err(|e| {
                    Error::storage(format!("Failed to parse token for key '{}': {}", key, e))
                })?;
                Ok(Some(token))
            }
            None => Ok(None),
        }
    }

    /// Set a token by key.
    fn set_token(&mut self, key: &str, token: &TokenInfo) -> Result<()> {
        let value = serde_json::to_value(token)?;
        self.tokens.insert(key.to_string(), value);
        Ok(())
    }

    /// Check if a key exists.
    fn contains_key(&self, key: &str) -> bool {
        self.tokens.contains_key(key)
    }
}

/// File-based token storage.
///
/// Stores OAuth tokens in a JSON file with secure permissions.
/// The file format uses a key-value structure to allow for
/// potential future multi-provider support.
///
/// # File Format
///
/// ```json
/// {
///   "anthropic": {
///     "token_type": "oauth",
///     "access_token": "...",
///     "refresh_token": "...|project_id|managed_project_id",
///     "expires_at": 1234567890
///   }
/// }
/// ```
///
/// # Security
///
/// - File permissions are set to 0600 (owner read/write only)
/// - Parent directories are created with 0700 permissions
/// - Uses temp file + rename for atomic writes
///
/// # Example
///
/// ```rust,ignore
/// use antigravity_gate::storage::FileTokenStorage;
///
/// // Use default path (~/.config/antigravity-gate/auth.json)
/// let storage = FileTokenStorage::default_path()?;
///
/// // Or specify a custom path
/// let storage = FileTokenStorage::new("~/my-tokens.json")?;
/// ```
#[derive(Debug, Clone)]
pub struct FileTokenStorage {
    /// Expanded path to the token file.
    path: PathBuf,
}

impl FileTokenStorage {
    /// Create a new FileTokenStorage with the specified path.
    ///
    /// The path can include `~` which will be expanded to the user's
    /// home directory.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the token file (supports `~` expansion)
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The path contains `~` but the home directory cannot be determined
    /// - The path is invalid
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use antigravity_gate::storage::FileTokenStorage;
    ///
    /// let storage = FileTokenStorage::new("~/.config/myapp/tokens.json")?;
    /// ```
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = expand_tilde(path.as_ref())?;
        Ok(Self { path })
    }

    /// Create a FileTokenStorage using the default path.
    ///
    /// Default path is `~/.config/antigravity-gate/auth.json`.
    ///
    /// # Errors
    ///
    /// Returns an error if the home directory cannot be determined.
    pub fn default_path() -> Result<Self> {
        let home =
            dirs::home_dir().ok_or_else(|| Error::config("Cannot determine home directory"))?;
        let path = home.join(CONFIG_DIR).join(TOKEN_FILE);
        Ok(Self { path })
    }

    /// Get the path to the token file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Read the token file if it exists.
    async fn read_file(&self) -> Result<Option<TokenFile>> {
        if !self.path.exists() {
            return Ok(None);
        }

        let content = tokio::fs::read_to_string(&self.path).await.map_err(|e| {
            Error::storage(format!(
                "Failed to read token file '{}': {}",
                self.path.display(),
                e
            ))
        })?;

        if content.trim().is_empty() {
            return Ok(None);
        }

        let file: TokenFile = serde_json::from_str(&content).map_err(|e| {
            Error::storage(format!(
                "Failed to parse token file '{}': {}",
                self.path.display(),
                e
            ))
        })?;

        Ok(Some(file))
    }

    /// Write the token file with secure permissions.
    #[instrument(skip(self, file))]
    async fn write_file(&self, file: &TokenFile) -> Result<()> {
        // Ensure parent directory exists
        if let Some(parent) = self.path.parent() {
            if !parent.exists() {
                tokio::fs::create_dir_all(parent).await.map_err(|e| {
                    Error::storage(format!(
                        "Failed to create directory '{}': {}",
                        parent.display(),
                        e
                    ))
                })?;

                // Set directory permissions on Unix
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let perms = std::fs::Permissions::from_mode(DIR_MODE);
                    tokio::fs::set_permissions(parent, perms)
                        .await
                        .map_err(|e| {
                            Error::storage(format!(
                                "Failed to set directory permissions on '{}': {}",
                                parent.display(),
                                e
                            ))
                        })?;
                }
            }
        }

        // Serialize token
        let content = serde_json::to_string_pretty(&file)?;

        // Write to temp file first, then rename for atomicity
        let temp_path = self.path.with_extension("tmp");
        tokio::fs::write(&temp_path, &content).await.map_err(|e| {
            Error::storage(format!(
                "Failed to write temp file '{}': {}",
                temp_path.display(),
                e
            ))
        })?;

        // Set file permissions on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(FILE_MODE);
            tokio::fs::set_permissions(&temp_path, perms)
                .await
                .map_err(|e| {
                    Error::storage(format!(
                        "Failed to set file permissions on '{}': {}",
                        temp_path.display(),
                        e
                    ))
                })?;
        }

        // Atomic rename
        tokio::fs::rename(&temp_path, &self.path)
            .await
            .map_err(|e| {
                Error::storage(format!(
                    "Failed to rename '{}' to '{}': {}",
                    temp_path.display(),
                    self.path.display(),
                    e
                ))
            })?;

        Ok(())
    }
}

#[async_trait]
impl TokenStorage for FileTokenStorage {
    #[instrument(skip(self))]
    async fn load(&self) -> Result<Option<TokenInfo>> {
        let file = self.read_file().await?;
        match file {
            Some(f) => f.get_token(TOKEN_KEY),
            None => Ok(None),
        }
    }

    #[instrument(skip(self, token))]
    async fn save(&self, token: &TokenInfo) -> Result<()> {
        // Load existing file to preserve other keys (if any)
        let mut file = self.read_file().await?.unwrap_or_default();
        file.set_token(TOKEN_KEY, token)?;
        self.write_file(&file).await
    }

    #[instrument(skip(self))]
    async fn remove(&self) -> Result<()> {
        if self.path.exists() {
            tokio::fs::remove_file(&self.path).await.map_err(|e| {
                Error::storage(format!(
                    "Failed to remove token file '{}': {}",
                    self.path.display(),
                    e
                ))
            })?;
        }
        Ok(())
    }

    async fn exists(&self) -> Result<bool> {
        if !self.path.exists() {
            return Ok(false);
        }

        // File exists, check if it has our key
        let file = self.read_file().await?;
        Ok(file.map(|f| f.contains_key(TOKEN_KEY)).unwrap_or(false))
    }

    fn name(&self) -> &str {
        "file"
    }
}

/// Expand `~` prefix to user's home directory.
fn expand_tilde(path: &Path) -> Result<PathBuf> {
    let path_str = path.to_string_lossy();
    if let Some(rest) = path_str.strip_prefix('~') {
        let home =
            dirs::home_dir().ok_or_else(|| Error::config("Cannot determine home directory"))?;
        if rest.is_empty() {
            Ok(home)
        } else {
            // Handle ~/something - strip the leading / if present
            let rest = rest.strip_prefix('/').unwrap_or(rest);
            Ok(home.join(rest))
        }
    } else {
        Ok(path.to_path_buf())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_save_and_load() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("tokens.json");
        let storage = FileTokenStorage::new(&path).unwrap();

        // Initially empty
        assert!(storage.load().await.unwrap().is_none());
        assert!(!storage.exists().await.unwrap());

        // Save a token
        let token = TokenInfo::new("access".into(), "refresh".into(), 3600);
        storage.save(&token).await.unwrap();

        // Load it back
        let loaded = storage.load().await.unwrap().unwrap();
        assert_eq!(loaded.access_token, "access");
        assert_eq!(loaded.refresh_token, "refresh");
        assert!(storage.exists().await.unwrap());
    }

    #[tokio::test]
    async fn test_remove() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("tokens.json");
        let storage = FileTokenStorage::new(&path).unwrap();

        // Save and then remove
        let token = TokenInfo::new("access".into(), "refresh".into(), 3600);
        storage.save(&token).await.unwrap();
        assert!(storage.exists().await.unwrap());

        storage.remove().await.unwrap();
        assert!(!storage.exists().await.unwrap());
        assert!(storage.load().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_remove_nonexistent() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nonexistent.json");
        let storage = FileTokenStorage::new(&path).unwrap();

        // Should not error
        storage.remove().await.unwrap();
    }

    #[tokio::test]
    async fn test_composite_token_round_trip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("tokens.json");
        let storage = FileTokenStorage::new(&path).unwrap();

        // Create token with project IDs
        let token = TokenInfo::new("access".into(), "refresh".into(), 3600)
            .with_project_ids("proj-123", Some("managed-456"));
        storage.save(&token).await.unwrap();

        // Load and verify
        let loaded = storage.load().await.unwrap().unwrap();
        let (base, project, managed) = loaded.parse_refresh_parts();
        assert_eq!(base, "refresh");
        assert_eq!(project.as_deref(), Some("proj-123"));
        assert_eq!(managed.as_deref(), Some("managed-456"));
    }

    #[tokio::test]
    async fn test_creates_parent_directories() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nested").join("dir").join("tokens.json");
        let storage = FileTokenStorage::new(&path).unwrap();

        let token = TokenInfo::new("access".into(), "refresh".into(), 3600);
        storage.save(&token).await.unwrap();

        // Verify file was created
        assert!(path.exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_file_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().unwrap();
        let path = dir.path().join("tokens.json");
        let storage = FileTokenStorage::new(&path).unwrap();

        let token = TokenInfo::new("access".into(), "refresh".into(), 3600);
        storage.save(&token).await.unwrap();

        let metadata = std::fs::metadata(&path).unwrap();
        let mode = metadata.permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "File permissions should be 0600");
    }

    #[test]
    fn test_expand_tilde() {
        // Test ~ alone
        let result = expand_tilde(Path::new("~")).unwrap();
        assert!(result.is_absolute());
        assert!(!result.to_string_lossy().contains('~'));

        // Test ~/path
        let result = expand_tilde(Path::new("~/test/path")).unwrap();
        assert!(result.is_absolute());
        assert!(result.ends_with("test/path"));

        // Test absolute path (no expansion)
        let result = expand_tilde(Path::new("/absolute/path")).unwrap();
        assert_eq!(result, Path::new("/absolute/path"));

        // Test relative path (no expansion)
        let result = expand_tilde(Path::new("relative/path")).unwrap();
        assert_eq!(result, Path::new("relative/path"));
    }

    #[test]
    fn test_default_path() {
        let storage = FileTokenStorage::default_path().unwrap();
        let path = storage.path();

        assert!(path.is_absolute());
        assert!(path.to_string_lossy().contains("antigravity-gate"));
        assert!(path.to_string_lossy().contains("auth.json"));
    }

    #[tokio::test]
    async fn test_storage_name() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("tokens.json");
        let storage = FileTokenStorage::new(&path).unwrap();
        assert_eq!(storage.name(), "file");
    }

    #[tokio::test]
    async fn test_empty_file_returns_none() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("tokens.json");

        // Create empty file
        tokio::fs::write(&path, "").await.unwrap();

        let storage = FileTokenStorage::new(&path).unwrap();
        assert!(storage.load().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_whitespace_file_returns_none() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("tokens.json");

        // Create file with only whitespace
        tokio::fs::write(&path, "   \n\t  \n").await.unwrap();

        let storage = FileTokenStorage::new(&path).unwrap();
        assert!(storage.load().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_invalid_json_returns_error() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("tokens.json");

        // Create file with invalid JSON
        tokio::fs::write(&path, "not valid json").await.unwrap();

        let storage = FileTokenStorage::new(&path).unwrap();
        let result = storage.load().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_file_without_anthropic_key() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("tokens.json");

        // Create file with different key
        tokio::fs::write(&path, r#"{"other_provider": {}}"#)
            .await
            .unwrap();

        let storage = FileTokenStorage::new(&path).unwrap();
        assert!(storage.load().await.unwrap().is_none());
        assert!(!storage.exists().await.unwrap());
    }
}
