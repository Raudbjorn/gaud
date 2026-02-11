//! System keyring-based token storage (feature-gated).

use async_trait::async_trait;
use tracing::debug;

use super::TokenStorage;
use crate::error::{Error, Result};
use crate::models::auth::KiroTokenInfo;

const SERVICE_NAME: &str = "kiro-gateway";

/// Token storage using the system keyring (Secret Service / Keychain / Credential Manager).
pub struct KeyringTokenStorage;

impl KeyringTokenStorage {
    /// Create a new keyring storage.
    pub fn new() -> Self {
        Self
    }
}

impl Default for KeyringTokenStorage {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl TokenStorage for KeyringTokenStorage {
    async fn load(&self, provider: &str) -> Result<Option<KiroTokenInfo>> {
        let entry =
            keyring::Entry::new(SERVICE_NAME, provider).map_err(|e| Error::Keyring(e.to_string()))?;
        match entry.get_password() {
            Ok(json) => {
                let token: KiroTokenInfo =
                    serde_json::from_str(&json).map_err(|e| Error::StorageSerialization(e.to_string()))?;
                debug!(provider, "Token loaded from keyring");
                Ok(Some(token))
            }
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(Error::Keyring(e.to_string())),
        }
    }

    async fn save(&self, provider: &str, token: &KiroTokenInfo) -> Result<()> {
        let json =
            serde_json::to_string(token).map_err(|e| Error::StorageSerialization(e.to_string()))?;
        let entry =
            keyring::Entry::new(SERVICE_NAME, provider).map_err(|e| Error::Keyring(e.to_string()))?;
        entry
            .set_password(&json)
            .map_err(|e| Error::Keyring(e.to_string()))?;
        debug!(provider, "Token saved to keyring");
        Ok(())
    }

    async fn remove(&self, provider: &str) -> Result<()> {
        let entry =
            keyring::Entry::new(SERVICE_NAME, provider).map_err(|e| Error::Keyring(e.to_string()))?;
        match entry.delete_credential() {
            Ok(()) => {
                debug!(provider, "Token removed from keyring");
                Ok(())
            }
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(Error::Keyring(e.to_string())),
        }
    }

    fn name(&self) -> &str {
        "keyring"
    }
}
