//! Keyring-based token storage.

use super::TokenStorage;
use crate::auth::error::AuthError;
use crate::auth::tokens::TokenInfo;
use tracing::instrument;

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
    fn entry(&self, provider: &str) -> Result<keyring::Entry, AuthError> {
        keyring::Entry::new(&self.service, provider)
            .map_err(|e| AuthError::Storage(format!("Failed to create keyring entry: {}", e)))
    }
}

#[cfg(feature = "system-keyring")]
impl TokenStorage for KeyringTokenStorage {
    #[instrument(skip(self))]
    fn load(&self, provider: &str) -> Result<Option<TokenInfo>, AuthError> {
        let entry = self.entry(provider)?;
        match entry.get_password() {
            Ok(password) => {
                let token: TokenInfo = serde_json::from_str(&password).map_err(|e| {
                    AuthError::Storage(format!("Failed to parse token from keyring: {}", e))
                })?;
                Ok(Some(token))
            }
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(AuthError::Storage(format!("Keyring error: {}", e))),
        }
    }

    #[instrument(skip(self, token))]
    fn save(&self, provider: &str, token: &TokenInfo) -> Result<(), AuthError> {
        let entry = self.entry(provider)?;
        let json = serde_json::to_string(token)
            .map_err(|e| AuthError::Storage(format!("Failed to serialize token: {}", e)))?;
        entry
            .set_password(&json)
            .map_err(|e| AuthError::Storage(format!("Keyring error: {}", e)))?;
        Ok(())
    }

    #[instrument(skip(self))]
    fn remove(&self, provider: &str) -> Result<(), AuthError> {
        let entry = self.entry(provider)?;
        match entry.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(AuthError::Storage(format!("Keyring error: {}", e))),
        }
    }

    fn name(&self) -> &str {
        "keyring"
    }
}
