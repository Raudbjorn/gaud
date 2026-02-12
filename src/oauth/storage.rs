//! Token storage backends for persisting OAuth credentials.
//!
//! Provides the [`TokenStorage`] trait and several implementations:
//! - [`FileTokenStorage`] - Stores tokens as individual JSON files per provider
//! - [`MemoryTokenStorage`] - In-memory storage for testing
//! - [`KeyringTokenStorage`] - System keyring storage (requires `system-keyring` feature)
//!
//! All storage operations are synchronous and take a `provider` parameter to
//! support multiple LLM providers in a single storage backend.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use tracing::instrument;

use super::OAuthError;
use super::token::TokenInfo;

// =============================================================================
// TokenStorage trait
// =============================================================================

/// Trait for token storage backends.
///
/// All storage implementations must be thread-safe (`Send + Sync`).
/// Operations take a `provider` parameter (e.g., "claude", "gemini") to
/// support storing tokens for multiple LLM providers.
pub trait TokenStorage: Send + Sync {
    /// Load the stored token for a provider, if any.
    fn load(&self, provider: &str) -> Result<Option<TokenInfo>, OAuthError>;

    /// Save a token for a provider to storage.
    fn save(&self, provider: &str, token: &TokenInfo) -> Result<(), OAuthError>;

    /// Remove the stored token for a provider.
    fn remove(&self, provider: &str) -> Result<(), OAuthError>;

    /// Check if a token exists in storage for a provider.
    fn exists(&self, provider: &str) -> Result<bool, OAuthError> {
        Ok(self.load(provider)?.is_some())
    }

    /// Get the name of this storage backend.
    fn name(&self) -> &str;
}

// Blanket implementation for Arc<T>
impl<T: TokenStorage + ?Sized> TokenStorage for Arc<T> {
    fn load(&self, provider: &str) -> Result<Option<TokenInfo>, OAuthError> {
        (**self).load(provider)
    }
    fn save(&self, provider: &str, token: &TokenInfo) -> Result<(), OAuthError> {
        (**self).save(provider, token)
    }
    fn remove(&self, provider: &str) -> Result<(), OAuthError> {
        (**self).remove(provider)
    }
    fn exists(&self, provider: &str) -> Result<bool, OAuthError> {
        (**self).exists(provider)
    }
    fn name(&self) -> &str {
        (**self).name()
    }
}

// Blanket implementation for Box<T>
impl<T: TokenStorage + ?Sized> TokenStorage for Box<T> {
    fn load(&self, provider: &str) -> Result<Option<TokenInfo>, OAuthError> {
        (**self).load(provider)
    }
    fn save(&self, provider: &str, token: &TokenInfo) -> Result<(), OAuthError> {
        (**self).save(provider, token)
    }
    fn remove(&self, provider: &str) -> Result<(), OAuthError> {
        (**self).remove(provider)
    }
    fn exists(&self, provider: &str) -> Result<bool, OAuthError> {
        (**self).exists(provider)
    }
    fn name(&self) -> &str {
        (**self).name()
    }
}

// =============================================================================
// FileTokenStorage
// =============================================================================

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
    fn provider_path(&self, provider: &str) -> PathBuf {
        self.dir.join(format!("{}.json", provider))
    }

    /// Ensure the storage directory exists with correct permissions.
    fn ensure_dir(&self) -> Result<(), OAuthError> {
        if !self.dir.exists() {
            std::fs::create_dir_all(&self.dir).map_err(|e| {
                OAuthError::Storage(format!(
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
                    OAuthError::Storage(format!(
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
    fn load(&self, provider: &str) -> Result<Option<TokenInfo>, OAuthError> {
        let path = self.provider_path(provider);
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                return Err(OAuthError::Storage(format!(
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
            OAuthError::Storage(format!(
                "Failed to parse token file '{}': {}",
                path.display(),
                e
            ))
        })?;

        Ok(Some(token))
    }

    #[instrument(skip(self, token))]
    fn save(&self, provider: &str, token: &TokenInfo) -> Result<(), OAuthError> {
        self.ensure_dir()?;

        let path = self.provider_path(provider);
        let content = serde_json::to_string_pretty(token)
            .map_err(|e| OAuthError::Storage(format!("Failed to serialize token: {}", e)))?;

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
                    OAuthError::Storage(format!(
                        "Failed to create temp file '{}': {}",
                        temp_path.display(),
                        e
                    ))
                })?;
            file.write_all(content.as_bytes()).map_err(|e| {
                OAuthError::Storage(format!(
                    "Failed to write temp file '{}': {}",
                    temp_path.display(),
                    e
                ))
            })?;
            file.sync_all().map_err(|e| {
                OAuthError::Storage(format!(
                    "Failed to sync temp file '{}': {}",
                    temp_path.display(),
                    e
                ))
            })?;
        }

        #[cfg(not(unix))]
        {
            std::fs::write(&temp_path, &content).map_err(|e| {
                OAuthError::Storage(format!(
                    "Failed to write temp file '{}': {}",
                    temp_path.display(),
                    e
                ))
            })?;
        }

        // Atomic rename
        if let Err(e) = std::fs::rename(&temp_path, &path) {
            let _ = std::fs::remove_file(&temp_path);
            return Err(OAuthError::Storage(format!(
                "Failed to rename '{}' to '{}': {}",
                temp_path.display(),
                path.display(),
                e
            )));
        }

        Ok(())
    }

    #[instrument(skip(self))]
    fn remove(&self, provider: &str) -> Result<(), OAuthError> {
        let path = self.provider_path(provider);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(OAuthError::Storage(format!(
                "Failed to remove token file '{}': {}",
                path.display(),
                e
            ))),
        }
    }

    fn exists(&self, provider: &str) -> Result<bool, OAuthError> {
        Ok(self.provider_path(provider).exists())
    }

    fn name(&self) -> &str {
        "file"
    }
}

// =============================================================================
// KeyringTokenStorage
// =============================================================================

/// Keyring-based token storage.
///
/// Uses the system's native credential store for secure token storage.
/// Tokens are serialized to JSON before storage.
///
/// Feature-gated behind `system-keyring`.
#[cfg(feature = "system-keyring")]
#[derive(Debug, Clone)]
pub struct KeyringTokenStorage {
    /// Service name for keyring entries.
    service: String,
}

#[cfg(feature = "system-keyring")]
impl Default for KeyringTokenStorage {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "system-keyring")]
impl KeyringTokenStorage {
    /// Service name prefix for keyring entries.
    const SERVICE_NAME: &str = "gaud-llm-proxy";

    /// Create a new KeyringTokenStorage with default service name.
    pub fn new() -> Self {
        Self {
            service: Self::SERVICE_NAME.to_string(),
        }
    }

    /// Create a KeyringTokenStorage with a custom service name.
    pub fn with_service(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }

    /// Check if the system keyring is available.
    pub fn is_available() -> bool {
        match keyring::Entry::new("gaud-test", "availability-check") {
            Ok(entry) => match entry.get_password() {
                Ok(_) => true,
                Err(keyring::Error::NoEntry) => true,
                Err(keyring::Error::NoStorageAccess(_)) => false,
                Err(keyring::Error::PlatformFailure(_)) => false,
                Err(_) => true,
            },
            Err(_) => false,
        }
    }

    /// Get the keyring entry for a provider.
    fn entry(&self, provider: &str) -> Result<keyring::Entry, OAuthError> {
        keyring::Entry::new(&self.service, provider)
            .map_err(|e| OAuthError::Storage(format!("Failed to create keyring entry: {}", e)))
    }
}

#[cfg(feature = "system-keyring")]
impl TokenStorage for KeyringTokenStorage {
    #[instrument(skip(self))]
    fn load(&self, provider: &str) -> Result<Option<TokenInfo>, OAuthError> {
        let entry = self.entry(provider)?;
        match entry.get_password() {
            Ok(password) => {
                let token: TokenInfo = serde_json::from_str(&password).map_err(|e| {
                    OAuthError::Storage(format!("Failed to parse token from keyring: {}", e))
                })?;
                Ok(Some(token))
            }
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(OAuthError::Storage(format!("Keyring error: {}", e))),
        }
    }

    #[instrument(skip(self, token))]
    fn save(&self, provider: &str, token: &TokenInfo) -> Result<(), OAuthError> {
        let entry = self.entry(provider)?;
        let json = serde_json::to_string(token)
            .map_err(|e| OAuthError::Storage(format!("Failed to serialize token: {}", e)))?;
        entry
            .set_password(&json)
            .map_err(|e| OAuthError::Storage(format!("Keyring error: {}", e)))?;
        Ok(())
    }

    #[instrument(skip(self))]
    fn remove(&self, provider: &str) -> Result<(), OAuthError> {
        let entry = self.entry(provider)?;
        match entry.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(OAuthError::Storage(format!("Keyring error: {}", e))),
        }
    }

    fn name(&self) -> &str {
        "keyring"
    }
}

// =============================================================================
// MemoryTokenStorage
// =============================================================================

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
    fn load(&self, provider: &str) -> Result<Option<TokenInfo>, OAuthError> {
        let guard = self.inner.read().expect("lock poisoned");
        Ok(guard.get(provider).cloned())
    }

    #[instrument(skip(self, token))]
    fn save(&self, provider: &str, token: &TokenInfo) -> Result<(), OAuthError> {
        let mut guard = self.inner.write().expect("lock poisoned");
        guard.insert(provider.to_string(), token.clone());
        Ok(())
    }

    #[instrument(skip(self))]
    fn remove(&self, provider: &str) -> Result<(), OAuthError> {
        let mut guard = self.inner.write().expect("lock poisoned");
        guard.remove(provider);
        Ok(())
    }

    fn exists(&self, provider: &str) -> Result<bool, OAuthError> {
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

    // =========================================================================
    // MemoryTokenStorage tests
    // =========================================================================

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

    #[test]
    fn test_memory_remove_empty() {
        let storage = MemoryTokenStorage::new();
        storage.remove("nonexistent").unwrap();
    }

    #[test]
    fn test_memory_overwrite() {
        let storage = MemoryTokenStorage::new();
        let token1 = TokenInfo::new(
            "access1".into(),
            Some("refresh1".into()),
            Some(3600),
            "claude",
        );
        storage.save("claude", &token1).unwrap();
        let token2 = TokenInfo::new(
            "access2".into(),
            Some("refresh2".into()),
            Some(7200),
            "claude",
        );
        storage.save("claude", &token2).unwrap();
        let loaded = storage.load("claude").unwrap().unwrap();
        assert_eq!(loaded.access_token, "access2");
    }

    #[test]
    fn test_memory_multiple_providers() {
        let storage = MemoryTokenStorage::new();
        let t1 = TokenInfo::new(
            "claude_access".into(),
            Some("r1".into()),
            Some(3600),
            "claude",
        );
        let t2 = TokenInfo::new(
            "gemini_access".into(),
            Some("r2".into()),
            Some(3600),
            "gemini",
        );
        storage.save("claude", &t1).unwrap();
        storage.save("gemini", &t2).unwrap();
        assert_eq!(storage.len(), 2);
        assert_eq!(
            storage.load("claude").unwrap().unwrap().access_token,
            "claude_access"
        );
        assert_eq!(
            storage.load("gemini").unwrap().unwrap().access_token,
            "gemini_access"
        );
    }

    #[test]
    fn test_memory_clear() {
        let storage = MemoryTokenStorage::new();
        let token = TokenInfo::new(
            "access".into(),
            Some("refresh".into()),
            Some(3600),
            "claude",
        );
        storage.save("claude", &token).unwrap();
        storage.save("gemini", &token).unwrap();
        assert_eq!(storage.len(), 2);
        storage.clear();
        assert!(storage.is_empty());
    }

    #[test]
    fn test_memory_clone_shares_state() {
        let storage1 = MemoryTokenStorage::new();
        let storage2 = storage1.clone();
        let token = TokenInfo::new(
            "access".into(),
            Some("refresh".into()),
            Some(3600),
            "claude",
        );
        storage1.save("claude", &token).unwrap();
        let loaded = storage2.load("claude").unwrap().unwrap();
        assert_eq!(loaded.access_token, "access");
    }

    #[test]
    fn test_memory_name() {
        let storage = MemoryTokenStorage::new();
        assert_eq!(storage.name(), "memory");
    }

    // =========================================================================
    // Arc/Box blanket impl tests
    // =========================================================================

    #[test]
    fn test_arc_storage() {
        let storage = Arc::new(MemoryTokenStorage::new());
        let token = TokenInfo::new(
            "access".into(),
            Some("refresh".into()),
            Some(3600),
            "claude",
        );
        storage.save("claude", &token).unwrap();
        let loaded = storage.load("claude").unwrap().unwrap();
        assert_eq!(loaded.access_token, "access");
        assert_eq!(storage.name(), "memory");
    }

    #[test]
    fn test_box_dyn_storage() {
        let storage: Box<dyn TokenStorage> = Box::new(MemoryTokenStorage::new());
        let token = TokenInfo::new(
            "access".into(),
            Some("refresh".into()),
            Some(3600),
            "claude",
        );
        storage.save("claude", &token).unwrap();
        let loaded = storage.load("claude").unwrap().unwrap();
        assert_eq!(loaded.access_token, "access");
    }

    // =========================================================================
    // FileTokenStorage tests
    // =========================================================================

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

    #[test]
    fn test_file_multiple_providers() {
        let dir = tempfile::tempdir().unwrap();
        let storage = FileTokenStorage::new(dir.path());

        let t1 = TokenInfo::new(
            "claude_access".into(),
            Some("r1".into()),
            Some(3600),
            "claude",
        );
        let t2 = TokenInfo::new(
            "gemini_access".into(),
            Some("r2".into()),
            Some(3600),
            "gemini",
        );

        storage.save("claude", &t1).unwrap();
        storage.save("gemini", &t2).unwrap();

        assert_eq!(
            storage.load("claude").unwrap().unwrap().access_token,
            "claude_access"
        );
        assert_eq!(
            storage.load("gemini").unwrap().unwrap().access_token,
            "gemini_access"
        );
    }

    #[test]
    fn test_file_remove() {
        let dir = tempfile::tempdir().unwrap();
        let storage = FileTokenStorage::new(dir.path());

        let token = TokenInfo::new(
            "access".into(),
            Some("refresh".into()),
            Some(3600),
            "claude",
        );
        storage.save("claude", &token).unwrap();
        assert!(storage.exists("claude").unwrap());

        storage.remove("claude").unwrap();
        assert!(!storage.exists("claude").unwrap());
    }

    #[test]
    fn test_file_remove_nonexistent() {
        let dir = tempfile::tempdir().unwrap();
        let storage = FileTokenStorage::new(dir.path());
        storage.remove("nonexistent").unwrap();
    }

    #[test]
    fn test_file_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let storage = FileTokenStorage::new(dir.path());

        let t1 = TokenInfo::new(
            "access1".into(),
            Some("refresh1".into()),
            Some(3600),
            "claude",
        );
        storage.save("claude", &t1).unwrap();

        let t2 = TokenInfo::new(
            "access2".into(),
            Some("refresh2".into()),
            Some(7200),
            "claude",
        );
        storage.save("claude", &t2).unwrap();

        let loaded = storage.load("claude").unwrap().unwrap();
        assert_eq!(loaded.access_token, "access2");
    }

    #[test]
    fn test_file_creates_parent_directories() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("nested").join("dir");
        let storage = FileTokenStorage::new(&nested);

        let token = TokenInfo::new(
            "access".into(),
            Some("refresh".into()),
            Some(3600),
            "claude",
        );
        storage.save("claude", &token).unwrap();

        assert!(nested.join("claude.json").exists());
    }

    #[cfg(unix)]
    #[test]
    fn test_file_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let storage = FileTokenStorage::new(dir.path());

        let token = TokenInfo::new(
            "access".into(),
            Some("refresh".into()),
            Some(3600),
            "claude",
        );
        storage.save("claude", &token).unwrap();

        let path = dir.path().join("claude.json");
        let metadata = std::fs::metadata(&path).unwrap();
        let mode = metadata.permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "File permissions should be 0600");
    }

    #[test]
    fn test_file_name() {
        let storage = FileTokenStorage::new("/tmp/test-tokens");
        assert_eq!(storage.name(), "file");
    }

    // =========================================================================
    // KeyringTokenStorage tests
    // =========================================================================

    #[cfg(feature = "system-keyring")]
    #[test]
    fn test_keyring_new() {
        let storage = KeyringTokenStorage::new();
        assert_eq!(storage.name(), "keyring");
    }

    #[cfg(feature = "system-keyring")]
    #[test]
    fn test_keyring_is_available() {
        // Just ensure it does not panic.
        let _available = KeyringTokenStorage::is_available();
    }
}
