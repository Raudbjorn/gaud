//! Callback-based token storage for custom storage backends.
//!
//! This module provides [`CallbackStorage`] which allows implementing
//! custom token storage using async closures. Also provides helper
//! sources for common patterns like file and environment variable access.

use async_trait::async_trait;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use tracing::instrument;

use super::TokenStorage;
use crate::auth::gemini::TokenInfo;
use crate::gemini::error::{Error, Result};

/// Type alias for async load callback.
///
/// Returns `Result<Option<TokenInfo>>`:
/// - `Ok(Some(token))` if token exists
/// - `Ok(None)` if no token stored
/// - `Err(_)` on storage error
pub type LoadCallback =
    Arc<dyn Fn() -> Pin<Box<dyn Future<Output = Result<Option<TokenInfo>>> + Send>> + Send + Sync>;

/// Type alias for async save callback.
///
/// Takes a `TokenInfo` reference and saves it to storage.
pub type SaveCallback =
    Arc<dyn Fn(TokenInfo) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> + Send + Sync>;

/// Type alias for async remove callback.
///
/// Removes any stored token. Should not error if no token exists.
pub type RemoveCallback =
    Arc<dyn Fn() -> Pin<Box<dyn Future<Output = Result<()>> + Send>> + Send + Sync>;

/// Callback-based token storage.
///
/// Allows implementing custom storage backends using async closures.
/// Each operation (load, save, remove) is handled by a separate callback,
/// giving full flexibility in how and where tokens are stored.
///
/// # Example
///
/// ```rust,ignore
/// use gaud::gemini::storage::CallbackStorage;
/// use gaud::gemini::TokenInfo;
/// use std::sync::Arc;
///
/// // Create storage with custom callbacks
/// let storage = CallbackStorage::new(
///     Arc::new(|| Box::pin(async { Ok(None) })),
///     Arc::new(|token| Box::pin(async move { Ok(()) })),
///     Arc::new(|| Box::pin(async { Ok(()) })),
///     "custom",
/// );
/// ```
#[derive(Clone)]
pub struct CallbackStorage {
    load: LoadCallback,
    save: SaveCallback,
    remove: RemoveCallback,
    storage_name: String,
}

impl std::fmt::Debug for CallbackStorage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CallbackStorage")
            .field("name", &self.storage_name)
            .finish()
    }
}

impl CallbackStorage {
    /// Create a new CallbackStorage with the given callbacks.
    ///
    /// # Arguments
    ///
    /// * `load` - Callback to load the token
    /// * `save` - Callback to save a token
    /// * `remove` - Callback to remove the token
    /// * `name` - Name of this storage backend (for debugging)
    pub fn new(
        load: LoadCallback,
        save: SaveCallback,
        remove: RemoveCallback,
        name: impl Into<String>,
    ) -> Self {
        Self {
            load,
            save,
            remove,
            storage_name: name.into(),
        }
    }

    /// Create a CallbackStorage from a FileSource.
    ///
    /// Convenience method for file-based callback storage.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use gaud::gemini::storage::{CallbackStorage, FileSource};
    ///
    /// let source = FileSource::new("~/.myapp/token.json")?;
    /// let storage = CallbackStorage::from_file_source(source);
    /// ```
    pub fn from_file_source(source: FileSource) -> Self {
        let source = Arc::new(source);

        let load_source = Arc::clone(&source);
        let load: LoadCallback = Arc::new(move || {
            let source = Arc::clone(&load_source);
            Box::pin(async move { source.load().await })
        });

        let save_source = Arc::clone(&source);
        let save: SaveCallback = Arc::new(move |token| {
            let source = Arc::clone(&save_source);
            Box::pin(async move { source.save(&token).await })
        });

        let remove_source = Arc::clone(&source);
        let remove: RemoveCallback = Arc::new(move || {
            let source = Arc::clone(&remove_source);
            Box::pin(async move { source.remove().await })
        });

        Self::new(load, save, remove, "file-callback")
    }

    /// Create a CallbackStorage from an EnvSource.
    ///
    /// Note: Environment-based storage is read-only; save and remove
    /// are no-ops that log warnings.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use gaud::gemini::storage::{CallbackStorage, EnvSource};
    ///
    /// let source = EnvSource::new("MY_APP_TOKEN");
    /// let storage = CallbackStorage::from_env_source(source);
    /// ```
    pub fn from_env_source(source: EnvSource) -> Self {
        let source = Arc::new(source);

        let load_source = Arc::clone(&source);
        let load: LoadCallback = Arc::new(move || {
            let source = Arc::clone(&load_source);
            Box::pin(async move { source.load() })
        });

        // Environment is read-only
        let save: SaveCallback = Arc::new(|_token| {
            Box::pin(async move {
                tracing::warn!("Attempted to save token to read-only environment storage");
                Ok(())
            })
        });

        let remove: RemoveCallback = Arc::new(|| {
            Box::pin(async move {
                tracing::warn!("Attempted to remove token from read-only environment storage");
                Ok(())
            })
        });

        Self::new(load, save, remove, "env-callback")
    }
}

#[async_trait]
impl TokenStorage for CallbackStorage {
    #[instrument(skip(self))]
    async fn load(&self) -> Result<Option<TokenInfo>> {
        (self.load)().await
    }

    #[instrument(skip(self, token))]
    async fn save(&self, token: &TokenInfo) -> Result<()> {
        (self.save)(token.clone()).await
    }

    #[instrument(skip(self))]
    async fn remove(&self) -> Result<()> {
        (self.remove)().await
    }

    fn name(&self) -> &str {
        &self.storage_name
    }
}

/// File-based token source for CallbackStorage.
///
/// Reads and writes tokens to a JSON file. Similar to FileTokenStorage
/// but designed to be used with CallbackStorage for more flexibility.
///
/// # Example
///
/// ```rust,ignore
/// use gaud::gemini::storage::FileSource;
///
/// let source = FileSource::new("~/.myapp/token.json")?;
/// let token = source.load().await?;
/// ```
#[derive(Debug, Clone)]
pub struct FileSource {
    path: PathBuf,
}

impl FileSource {
    /// Create a new FileSource with the given path.
    ///
    /// Supports `~` expansion for home directory.
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = expand_tilde(path.as_ref())?;
        Ok(Self { path })
    }

    /// Load token from file.
    pub async fn load(&self) -> Result<Option<TokenInfo>> {
        if !self.path.exists() {
            return Ok(None);
        }

        let content = tokio::fs::read_to_string(&self.path).await.map_err(|e| {
            Error::storage(format!(
                "Failed to read file '{}': {}",
                self.path.display(),
                e
            ))
        })?;

        if content.trim().is_empty() {
            return Ok(None);
        }

        let token: TokenInfo = serde_json::from_str(&content).map_err(|e| {
            Error::storage(format!(
                "Failed to parse token from '{}': {}",
                self.path.display(),
                e
            ))
        })?;

        Ok(Some(token))
    }

    /// Save token to file.
    pub async fn save(&self, token: &TokenInfo) -> Result<()> {
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
            }
        }

        let content = serde_json::to_string_pretty(token)?;
        tokio::fs::write(&self.path, &content).await.map_err(|e| {
            Error::storage(format!(
                "Failed to write file '{}': {}",
                self.path.display(),
                e
            ))
        })?;

        // Set permissions on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            tokio::fs::set_permissions(&self.path, perms)
                .await
                .map_err(|e| {
                    Error::storage(format!(
                        "Failed to set permissions on '{}': {}",
                        self.path.display(),
                        e
                    ))
                })?;
        }

        Ok(())
    }

    /// Remove the token file.
    pub async fn remove(&self) -> Result<()> {
        if self.path.exists() {
            tokio::fs::remove_file(&self.path).await.map_err(|e| {
                Error::storage(format!(
                    "Failed to remove file '{}': {}",
                    self.path.display(),
                    e
                ))
            })?;
        }
        Ok(())
    }

    /// Get the path to the token file.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Environment variable token source for CallbackStorage.
///
/// Reads token information from environment variables. This is a
/// read-only source; saving and removing are no-ops.
///
/// The token is expected to be in JSON format in the environment variable.
///
/// # Example
///
/// ```rust
/// use gaud::gemini::storage::EnvSource;
///
/// let source = EnvSource::new("MY_APP_TOKEN");
/// // If MY_APP_TOKEN contains valid TokenInfo JSON, it will be parsed
/// ```
#[derive(Debug, Clone)]
pub struct EnvSource {
    var_name: String,
}

impl EnvSource {
    /// Create a new EnvSource with the given environment variable name.
    ///
    /// The environment variable should contain a JSON-serialized TokenInfo.
    pub fn new(var_name: impl Into<String>) -> Self {
        Self {
            var_name: var_name.into(),
        }
    }

    /// Load token from environment variable.
    ///
    /// Returns `Ok(None)` if the variable is not set.
    /// Returns `Err` if the variable is set but cannot be parsed.
    pub fn load(&self) -> Result<Option<TokenInfo>> {
        match std::env::var(&self.var_name) {
            Ok(value) if !value.is_empty() => {
                let token: TokenInfo = serde_json::from_str(&value).map_err(|e| {
                    Error::storage(format!(
                        "Failed to parse token from env var '{}': {}",
                        self.var_name, e
                    ))
                })?;
                Ok(Some(token))
            }
            Ok(_) => Ok(None), // Empty value
            Err(std::env::VarError::NotPresent) => Ok(None),
            Err(e) => Err(Error::storage(format!(
                "Failed to read env var '{}': {}",
                self.var_name, e
            ))),
        }
    }

    /// Get the environment variable name.
    pub fn var_name(&self) -> &str {
        &self.var_name
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
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_callback_storage_basic() {
        let call_count = Arc::new(AtomicUsize::new(0));
        let stored = Arc::new(tokio::sync::RwLock::new(None::<TokenInfo>));

        let stored_load = Arc::clone(&stored);
        let load: LoadCallback = Arc::new(move || {
            let stored = Arc::clone(&stored_load);
            Box::pin(async move {
                let guard = stored.read().await;
                Ok(guard.clone())
            })
        });

        let stored_save = Arc::clone(&stored);
        let count_save = Arc::clone(&call_count);
        let save: SaveCallback = Arc::new(move |token| {
            let stored = Arc::clone(&stored_save);
            let count = Arc::clone(&count_save);
            Box::pin(async move {
                count.fetch_add(1, Ordering::SeqCst);
                let mut guard = stored.write().await;
                *guard = Some(token);
                Ok(())
            })
        });

        let stored_remove = Arc::clone(&stored);
        let remove: RemoveCallback = Arc::new(move || {
            let stored = Arc::clone(&stored_remove);
            Box::pin(async move {
                let mut guard = stored.write().await;
                *guard = None;
                Ok(())
            })
        });

        let storage = CallbackStorage::new(load, save, remove, "test");

        // Initially empty
        assert!(storage.load().await.unwrap().is_none());

        // Save a token
        let token = TokenInfo::new("access".into(), "refresh".into(), 3600);
        storage.save(&token).await.unwrap();

        // Verify save was called
        assert_eq!(call_count.load(Ordering::SeqCst), 1);

        // Load it back
        let loaded = storage.load().await.unwrap().unwrap();
        assert_eq!(loaded.access_token, "access");

        // Remove
        storage.remove().await.unwrap();
        assert!(storage.load().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_callback_storage_name() {
        let load: LoadCallback = Arc::new(|| Box::pin(async { Ok(None) }));
        let save: SaveCallback = Arc::new(|_| Box::pin(async { Ok(()) }));
        let remove: RemoveCallback = Arc::new(|| Box::pin(async { Ok(()) }));

        let storage = CallbackStorage::new(load, save, remove, "my-custom-storage");
        assert_eq!(storage.name(), "my-custom-storage");
    }

    #[tokio::test]
    async fn test_file_source() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("token.json");
        let source = FileSource::new(&path).unwrap();

        // Initially empty
        assert!(source.load().await.unwrap().is_none());

        // Save a token
        let token = TokenInfo::new("access".into(), "refresh".into(), 3600);
        source.save(&token).await.unwrap();

        // Load it back
        let loaded = source.load().await.unwrap().unwrap();
        assert_eq!(loaded.access_token, "access");

        // Remove
        source.remove().await.unwrap();
        assert!(source.load().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_file_source_creates_directories() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nested").join("dirs").join("token.json");
        let source = FileSource::new(&path).unwrap();

        let token = TokenInfo::new("access".into(), "refresh".into(), 3600);
        source.save(&token).await.unwrap();

        assert!(path.exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_file_source_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().unwrap();
        let path = dir.path().join("token.json");
        let source = FileSource::new(&path).unwrap();

        let token = TokenInfo::new("access".into(), "refresh".into(), 3600);
        source.save(&token).await.unwrap();

        let metadata = std::fs::metadata(&path).unwrap();
        let mode = metadata.permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[tokio::test]
    async fn test_from_file_source() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("token.json");
        let source = FileSource::new(&path).unwrap();
        let storage = CallbackStorage::from_file_source(source);

        assert_eq!(storage.name(), "file-callback");

        // Test save and load
        let token = TokenInfo::new("access".into(), "refresh".into(), 3600);
        storage.save(&token).await.unwrap();

        let loaded = storage.load().await.unwrap().unwrap();
        assert_eq!(loaded.access_token, "access");
    }

    #[test]
    fn test_env_source_not_set() {
        // Use a unique variable name that's unlikely to be set
        let source = EnvSource::new("ANTIGRAVITY_TEST_TOKEN_NOT_SET_12345");
        assert!(source.load().unwrap().is_none());
    }

    #[test]
    fn test_env_source_valid_token() {
        let var_name = "ANTIGRAVITY_TEST_TOKEN_VALID";
        let token = TokenInfo::new("access".into(), "refresh".into(), 3600);
        let json = serde_json::to_string(&token).unwrap();

        unsafe { std::env::set_var(var_name, &json); }
        let source = EnvSource::new(var_name);
        let loaded = source.load().unwrap().unwrap();
        assert_eq!(loaded.access_token, "access");

        unsafe { std::env::remove_var(var_name); }
    }

    #[test]
    fn test_env_source_invalid_json() {
        let var_name = "ANTIGRAVITY_TEST_TOKEN_INVALID";
        unsafe { std::env::set_var(var_name, "not valid json"); }

        let source = EnvSource::new(var_name);
        assert!(source.load().is_err());

        unsafe { std::env::remove_var(var_name); }
    }

    #[test]
    fn test_env_source_empty_value() {
        let var_name = "ANTIGRAVITY_TEST_TOKEN_EMPTY";
        unsafe { std::env::set_var(var_name, ""); }

        let source = EnvSource::new(var_name);
        assert!(source.load().unwrap().is_none());

        unsafe { std::env::remove_var(var_name); }
    }

    #[tokio::test]
    async fn test_from_env_source() {
        let var_name = "ANTIGRAVITY_TEST_CALLBACK_TOKEN";
        let token = TokenInfo::new("access".into(), "refresh".into(), 3600);
        let json = serde_json::to_string(&token).unwrap();
        unsafe { std::env::set_var(var_name, &json); }

        let source = EnvSource::new(var_name);
        let storage = CallbackStorage::from_env_source(source);

        assert_eq!(storage.name(), "env-callback");

        let loaded = storage.load().await.unwrap().unwrap();
        assert_eq!(loaded.access_token, "access");

        // Save and remove should be no-ops (read-only)
        storage.save(&token).await.unwrap();
        storage.remove().await.unwrap();

        // Token should still be loadable (wasn't actually removed)
        let loaded = storage.load().await.unwrap().unwrap();
        assert_eq!(loaded.access_token, "access");

        unsafe { std::env::remove_var(var_name); }
    }

    #[test]
    fn test_callback_storage_debug() {
        let load: LoadCallback = Arc::new(|| Box::pin(async { Ok(None) }));
        let save: SaveCallback = Arc::new(|_| Box::pin(async { Ok(()) }));
        let remove: RemoveCallback = Arc::new(|| Box::pin(async { Ok(()) }));

        let storage = CallbackStorage::new(load, save, remove, "test-debug");
        let debug = format!("{:?}", storage);
        assert!(debug.contains("test-debug"));
    }
}
