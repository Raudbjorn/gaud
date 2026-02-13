//! File-based token storage.

use super::TokenStorage;
use crate::auth::error::AuthError;
use crate::auth::tokens::TokenInfo;
use std::path::{Path, PathBuf};
use tracing::instrument;

/// File permissions for token files (Unix only): owner read/write.
#[cfg(unix)]
const FILE_MODE: u32 = 0o600;

/// Directory permissions (Unix only): owner read/write/execute.
#[cfg(unix)]
const DIR_MODE: u32 = 0o700;

/// File-based token storage.
///
/// Stores OAuth tokens as individual JSON files per provider in a configurable
/// directory. File path: `{dir}/{provider}.json`.
///
/// # Security
/// - File permissions are set to 0600 (owner read/write only) on Unix
/// - Parent directories are created with 0700 permissions
#[derive(Debug, Clone)]
pub struct FileTokenStorage {
    /// Directory where token files are stored.
    dir: PathBuf,
}

impl FileTokenStorage {
    /// Create a new FileTokenStorage with the specified directory.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// Get the directory where tokens are stored.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Get the file path for a specific provider.
    fn provider_path(&self, provider: &str) -> Result<PathBuf, AuthError> {
        if provider.is_empty() {
             return Err(AuthError::Storage("Provider name cannot be empty".to_string()));
        }

        // Reject path traversal and ensure safe filename
        if provider.contains('/') || provider.contains('\\') || provider.contains("..") {
             return Err(AuthError::Storage(format!("Invalid provider name '{}': potential path traversal", provider)));
        }

        // Allow only alphanumeric, hyphen, and underscore
        if !provider.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
             return Err(AuthError::Storage(format!("Invalid provider name '{}': contains invalid characters", provider)));
        }

        Ok(self.dir.join(format!("{}.json", provider)))
    }

    /// Ensure the storage directory exists with correct permissions.
    fn ensure_dir(&self) -> Result<(), AuthError> {
        if !self.dir.exists() {
            std::fs::create_dir_all(&self.dir).map_err(|e| {
                AuthError::Storage(format!(
                    "Failed to create token directory '{}': {}",
                    self.dir.display(),
                    e
                ))
            })?;

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let perms = std::fs::Permissions::from_mode(DIR_MODE);
                std::fs::set_permissions(&self.dir, perms).map_err(|e| {
                    AuthError::Storage(format!(
                        "Failed to set directory permissions on '{}': {}",
                        self.dir.display(),
                        e
                    ))
                })?;
            }
        }
        Ok(())
    }
}

impl TokenStorage for FileTokenStorage {
    #[instrument(skip(self))]
    fn load(&self, provider: &str) -> Result<Option<TokenInfo>, AuthError> {
        let path = self.provider_path(provider)?;
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                return Err(AuthError::Storage(format!(
                    "Failed to read token file '{}': {}",
                    path.display(),
                    e
                )));
            }
        };

        if content.trim().is_empty() {
            return Ok(None);
        }

        let token: TokenInfo = serde_json::from_str(&content).map_err(|e| {
            AuthError::Storage(format!(
                "Failed to parse token file '{}': {}",
                path.display(),
                e
            ))
        })?;

        Ok(Some(token))
    }

    #[instrument(skip(self, token))]
    fn save(&self, provider: &str, token: &TokenInfo) -> Result<(), AuthError> {
        self.ensure_dir()?;

        let path = self.provider_path(provider)?;
        let content = serde_json::to_string_pretty(token)
            .map_err(|e| AuthError::Storage(format!("Failed to serialize token: {}", e)))?;

        // Write to temp file first, then rename for atomicity.
        // On Unix, set 0600 permissions at creation time to avoid a window
        // where tokens are readable by other users.
        let temp_path = path.with_extension("tmp");

        #[cfg(unix)]
        {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(FILE_MODE)
                .open(&temp_path)
                .map_err(|e| {
                    AuthError::Storage(format!(
                        "Failed to create temp file '{}': {}",
                        temp_path.display(),
                        e
                    ))
                })?;
            file.write_all(content.as_bytes()).map_err(|e| {
                AuthError::Storage(format!(
                    "Failed to write temp file '{}': {}",
                    temp_path.display(),
                    e
                ))
            })?;
            file.sync_all().map_err(|e| {
                AuthError::Storage(format!(
                    "Failed to sync temp file '{}': {}",
                    temp_path.display(),
                    e
                ))
            })?;
        }

        #[cfg(not(unix))]
        {
            std::fs::write(&temp_path, &content).map_err(|e| {
                AuthError::Storage(format!(
                    "Failed to write temp file '{}': {}",
                    temp_path.display(),
                    e
                ))
            })?;
        }

        // Atomic rename
        if let Err(e) = std::fs::rename(&temp_path, &path) {
            let _ = std::fs::remove_file(&temp_path);
            return Err(AuthError::Storage(format!(
                "Failed to rename '{}' to '{}': {}",
                temp_path.display(),
                path.display(),
                e
            )));
        }

        Ok(())
    }

    #[instrument(skip(self))]
    fn remove(&self, provider: &str) -> Result<(), AuthError> {
        let path = self.provider_path(provider)?;
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(AuthError::Storage(format!(
                "Failed to remove token file '{}': {}",
                path.display(),
                e
            ))),
        }
    }

    fn exists(&self, provider: &str) -> Result<bool, AuthError> {
        Ok(self.provider_path(provider)?.exists())
    }

    fn name(&self) -> &str {
        "file"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let storage = FileTokenStorage::new(dir.path());

        assert!(storage.load("claude").unwrap().is_none());
        assert!(!storage.exists("claude").unwrap());

        let token = TokenInfo::new(
            "access".into(),
            Some("refresh".into()),
            Some(3600),
            "claude",
        );
        storage.save("claude", &token).unwrap();

        let loaded = storage.load("claude").unwrap().unwrap();
        assert_eq!(loaded.access_token, "access");
        assert!(storage.exists("claude").unwrap());
    }
}
