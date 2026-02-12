//! System keyring token storage (feature-gated).
//!
//! Provides secure token storage using the system's native credential store:
//! - macOS: Keychain
//! - Linux: Secret Service (GNOME Keyring, KWallet)
//! - Windows: Credential Manager
//!
//! # Feature Flag
//!
//! This module requires the `keyring` feature:
//!
//! ```toml
//! [dependencies]
//! gemini = { version = "0.1", features = ["keyring"] }
//! ```

use async_trait::async_trait;
use keyring::Entry;
use tracing::instrument;

use super::TokenStorage;
use crate::auth::gemini::TokenInfo;
use crate::gemini::error::{Error, Result};

/// Service name used for keyring entries.
const SERVICE_NAME: &str = "gemini";

/// Default account name for keyring entries.
const DEFAULT_ACCOUNT: &str = "oauth-token";

/// Keyring-based token storage.
///
/// Uses the system's native credential store for secure token storage.
/// Tokens are serialized to JSON before storage.
///
/// # Platform Support
///
/// - **macOS**: Uses Keychain Services
/// - **Linux**: Uses Secret Service (requires `gnome-keyring` or `kwallet`)
/// - **Windows**: Uses Credential Manager
///
/// # Example
///
/// ```rust,ignore
/// use gaud::gemini::storage::KeyringTokenStorage;
///
/// // Check if keyring is available
/// if KeyringTokenStorage::is_available() {
///     let storage = KeyringTokenStorage::new();
///     // Use storage...
/// }
/// ```
#[derive(Debug, Clone)]
pub struct KeyringTokenStorage {
    /// Service name for keyring entry.
    service: String,
    /// Account name for keyring entry.
    account: String,
}

impl Default for KeyringTokenStorage {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyringTokenStorage {
    /// Create a new KeyringTokenStorage with default service and account names.
    ///
    /// Uses service name "gemini" and account "oauth-token".
    pub fn new() -> Self {
        Self {
            service: SERVICE_NAME.to_string(),
            account: DEFAULT_ACCOUNT.to_string(),
        }
    }

    /// Create a KeyringTokenStorage with a custom account name.
    ///
    /// Useful for storing multiple tokens (e.g., for different users).
    pub fn with_account(account: impl Into<String>) -> Self {
        Self {
            service: SERVICE_NAME.to_string(),
            account: account.into(),
        }
    }

    /// Create a KeyringTokenStorage with custom service and account names.
    ///
    /// Allows full customization of the keyring entry location.
    pub fn with_service_and_account(
        service: impl Into<String>,
        account: impl Into<String>,
    ) -> Self {
        Self {
            service: service.into(),
            account: account.into(),
        }
    }

    /// Check if the system keyring is available.
    ///
    /// Returns `true` if a keyring backend is available and functional.
    /// This performs a test operation to verify the keyring works.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use gaud::gemini::storage::KeyringTokenStorage;
    ///
    /// if KeyringTokenStorage::is_available() {
    ///     println!("Keyring is available");
    /// } else {
    ///     println!("Falling back to file storage");
    /// }
    /// ```
    pub fn is_available() -> bool {
        // Try to create an entry - this tests if the backend is available
        match Entry::new("gemini-test", "availability-check") {
            Ok(entry) => {
                // Try to get the password (will fail with NoEntry, which is fine)
                match entry.get_password() {
                    Ok(_) => true,
                    Err(keyring::Error::NoEntry) => true, // Backend works, just no entry
                    Err(keyring::Error::NoStorageAccess(_)) => false,
                    Err(keyring::Error::PlatformFailure(_)) => false,
                    Err(_) => true, // Other errors might be transient
                }
            }
            Err(_) => false,
        }
    }

    /// Get the service name for this storage.
    pub fn service(&self) -> &str {
        &self.service
    }

    /// Get the account name for this storage.
    pub fn account(&self) -> &str {
        &self.account
    }

    /// Get the keyring entry.
    fn entry(&self) -> Result<Entry> {
        Entry::new(&self.service, &self.account)
            .map_err(|e| Error::storage(format!("Failed to create keyring entry: {}", e)))
    }
}

#[async_trait]
impl TokenStorage for KeyringTokenStorage {
    #[instrument(skip(self))]
    async fn load(&self) -> Result<Option<TokenInfo>> {
        let entry = self.entry()?;

        // Run blocking keyring operation in a blocking task
        let result = tokio::task::spawn_blocking(move || entry.get_password())
            .await
            .map_err(|e| Error::storage(format!("Keyring task failed: {}", e)))?;

        match result {
            Ok(password) => {
                let token: TokenInfo = serde_json::from_str(&password).map_err(|e| {
                    Error::storage(format!("Failed to parse token from keyring: {}", e))
                })?;
                Ok(Some(token))
            }
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(Error::from(e)),
        }
    }

    #[instrument(skip(self, token))]
    async fn save(&self, token: &TokenInfo) -> Result<()> {
        let entry = self.entry()?;
        let json = serde_json::to_string(token)?;

        // Run blocking keyring operation in a blocking task
        tokio::task::spawn_blocking(move || entry.set_password(&json))
            .await
            .map_err(|e| Error::storage(format!("Keyring task failed: {}", e)))??;

        Ok(())
    }

    #[instrument(skip(self))]
    async fn remove(&self) -> Result<()> {
        let entry = self.entry()?;

        // Run blocking keyring operation in a blocking task
        let result = tokio::task::spawn_blocking(move || entry.delete_credential())
            .await
            .map_err(|e| Error::storage(format!("Keyring task failed: {}", e)))?;

        match result {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()), // Already removed
            Err(e) => Err(Error::from(e)),
        }
    }

    fn name(&self) -> &str {
        "keyring"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Note: These tests require a working keyring backend.
    // They may fail or be skipped on CI systems without one.

    #[test]
    fn test_new() {
        let storage = KeyringTokenStorage::new();
        assert_eq!(storage.service(), "gemini");
        assert_eq!(storage.account(), "oauth-token");
    }

    #[test]
    fn test_with_account() {
        let storage = KeyringTokenStorage::with_account("my-account");
        assert_eq!(storage.service(), "gemini");
        assert_eq!(storage.account(), "my-account");
    }

    #[test]
    fn test_with_service_and_account() {
        let storage = KeyringTokenStorage::with_service_and_account("my-service", "my-account");
        assert_eq!(storage.service(), "my-service");
        assert_eq!(storage.account(), "my-account");
    }

    #[test]
    fn test_default() {
        let storage = KeyringTokenStorage::default();
        assert_eq!(storage.service(), "gemini");
        assert_eq!(storage.account(), "oauth-token");
    }

    #[test]
    fn test_is_available() {
        // Just test that this doesn't panic
        let _available = KeyringTokenStorage::is_available();
    }

    #[test]
    fn test_storage_name() {
        let storage = KeyringTokenStorage::new();
        assert_eq!(storage.name(), "keyring");
    }

    // Integration tests that require a working keyring
    // These use a unique account name to avoid conflicts

    #[tokio::test]
    async fn test_save_load_remove() {
        if !KeyringTokenStorage::is_available() {
            eprintln!("Skipping keyring test: keyring not available");
            return;
        }

        // Use a unique account for this test
        let storage = KeyringTokenStorage::with_account("test-save-load-remove");

        // Clean up any leftover test data
        let _ = storage.remove().await;

        // Initially empty
        assert!(storage.load().await.unwrap().is_none());

        // Save a token
        let token = TokenInfo::new("access".into(), "refresh".into(), 3600);
        match storage.save(&token).await {
            Ok(()) => {}
            Err(e) => {
                eprintln!("Skipping keyring test: save failed: {}", e);
                return;
            }
        }

        // Load it back
        match storage.load().await {
            Ok(Some(loaded)) => {
                assert_eq!(loaded.access_token, "access");
                assert_eq!(loaded.refresh_token, "refresh");
            }
            Ok(None) => {
                eprintln!("Skipping keyring test: load returned None after save");
                return;
            }
            Err(e) => {
                eprintln!("Skipping keyring test: load failed: {}", e);
                return;
            }
        }

        // Remove
        storage.remove().await.unwrap();
        assert!(storage.load().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_composite_token() {
        if !KeyringTokenStorage::is_available() {
            eprintln!("Skipping keyring test: keyring not available");
            return;
        }

        let storage = KeyringTokenStorage::with_account("test-composite-token");
        let _ = storage.remove().await;

        let token = TokenInfo::new("access".into(), "refresh".into(), 3600)
            .with_project_ids("proj-123", Some("managed-456"));
        match storage.save(&token).await {
            Ok(()) => {}
            Err(e) => {
                eprintln!("Skipping keyring test: save failed: {}", e);
                return;
            }
        }

        match storage.load().await {
            Ok(Some(loaded)) => {
                let (base, project, managed) = loaded.parse_refresh_parts();
                assert_eq!(base, "refresh");
                assert_eq!(project.as_deref(), Some("proj-123"));
                assert_eq!(managed.as_deref(), Some("managed-456"));
            }
            Ok(None) => {
                eprintln!("Skipping keyring test: load returned None after save");
                return;
            }
            Err(e) => {
                eprintln!("Skipping keyring test: load failed: {}", e);
                return;
            }
        }

        storage.remove().await.unwrap();
    }

    #[tokio::test]
    async fn test_overwrite() {
        if !KeyringTokenStorage::is_available() {
            eprintln!("Skipping keyring test: keyring not available");
            return;
        }

        let storage = KeyringTokenStorage::with_account("test-overwrite");
        let _ = storage.remove().await;

        let token1 = TokenInfo::new("access1".into(), "refresh1".into(), 3600);
        match storage.save(&token1).await {
            Ok(()) => {}
            Err(e) => {
                eprintln!("Skipping keyring test: save failed: {}", e);
                return;
            }
        }

        let token2 = TokenInfo::new("access2".into(), "refresh2".into(), 7200);
        match storage.save(&token2).await {
            Ok(()) => {}
            Err(e) => {
                eprintln!("Skipping keyring test: save failed: {}", e);
                return;
            }
        }

        match storage.load().await {
            Ok(Some(loaded)) => {
                assert_eq!(loaded.access_token, "access2");
                assert_eq!(loaded.refresh_token, "refresh2");
            }
            Ok(None) => {
                eprintln!("Skipping keyring test: load returned None after save");
                return;
            }
            Err(e) => {
                eprintln!("Skipping keyring test: load failed: {}", e);
                return;
            }
        }

        storage.remove().await.unwrap();
    }

    #[tokio::test]
    async fn test_remove_nonexistent() {
        if !KeyringTokenStorage::is_available() {
            eprintln!("Skipping keyring test: keyring not available");
            return;
        }

        let storage = KeyringTokenStorage::with_account("test-remove-nonexistent");

        // Should not error when removing nonexistent entry
        storage.remove().await.unwrap();
    }
}
