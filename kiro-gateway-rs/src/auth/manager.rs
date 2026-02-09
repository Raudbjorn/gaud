//! Token lifecycle manager.
//!
//! Handles credential loading, token refresh, caching, and thread-safe access.

use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::error::{Error, Result};
use crate::models::auth::{AuthType, KiroTokenInfo};
use crate::storage::TokenStorage;

use super::constants;

/// Manages the Kiro token lifecycle.
///
/// Thread-safe: uses `RwLock` internally so it can be shared across tasks.
pub struct KiroAuthManager {
    /// Current token state.
    token: Arc<RwLock<Option<KiroTokenInfo>>>,
    /// HTTP client for refresh requests.
    client: reqwest::Client,
    /// Machine fingerprint for User-Agent headers.
    fingerprint: String,
    /// Optional storage backend for persisting tokens.
    storage: Option<Arc<dyn TokenStorage>>,
    /// Provider identifier for storage.
    provider: String,
}

impl KiroAuthManager {
    /// Create a new auth manager with initial credentials.
    pub fn new(token_info: KiroTokenInfo) -> Self {
        Self {
            token: Arc::new(RwLock::new(Some(token_info))),
            client: reqwest::Client::new(),
            fingerprint: constants::machine_fingerprint(),
            storage: None,
            provider: "kiro".to_string(),
        }
    }

    /// Create an auth manager with no initial credentials.
    pub fn empty() -> Self {
        Self {
            token: Arc::new(RwLock::new(None)),
            client: reqwest::Client::new(),
            fingerprint: constants::machine_fingerprint(),
            storage: None,
            provider: "kiro".to_string(),
        }
    }

    /// Set the storage backend for token persistence.
    pub fn with_storage(mut self, storage: Arc<dyn TokenStorage>) -> Self {
        self.storage = Some(storage);
        self
    }

    /// Set the HTTP client (useful for testing or custom TLS config).
    pub fn with_client(mut self, client: reqwest::Client) -> Self {
        self.client = client;
        self
    }

    /// Set a custom fingerprint.
    pub fn with_fingerprint(mut self, fingerprint: String) -> Self {
        self.fingerprint = fingerprint;
        self
    }

    /// Get the machine fingerprint.
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    /// Get the provider identifier.
    pub fn provider(&self) -> &str {
        &self.provider
    }

    /// Set initial credentials.
    pub async fn set_credentials(&self, token_info: KiroTokenInfo) {
        let mut token = self.token.write().await;
        *token = Some(token_info);
    }

    /// Get a valid access token, refreshing if necessary.
    ///
    /// This is the primary method callers use. It:
    /// 1. Checks if a token exists
    /// 2. Refreshes proactively if within the threshold
    /// 3. Returns the current access token
    pub async fn get_access_token(&self) -> Result<String> {
        // First, try to get a valid cached token
        {
            let token = self.token.read().await;
            if let Some(info) = token.as_ref() {
                if !info.needs_refresh() && !info.access_token.is_empty() {
                    return Ok(info.access_token.clone());
                }
            }
        }

        // Need to refresh - acquire write lock
        self.refresh().await?;

        let token = self.token.read().await;
        token
            .as_ref()
            .map(|t| t.access_token.clone())
            .ok_or(Error::NotAuthenticated)
    }

    /// Get the current token info (read-only snapshot).
    pub async fn token_info(&self) -> Option<KiroTokenInfo> {
        self.token.read().await.clone()
    }

    /// Get the profile ARN from current credentials.
    pub async fn profile_arn(&self) -> Option<String> {
        self.token
            .read()
            .await
            .as_ref()
            .and_then(|t| t.profile_arn.clone())
    }

    /// Get the API region from current credentials.
    pub async fn region(&self) -> String {
        self.token
            .read()
            .await
            .as_ref()
            .map(|t| t.region.clone())
            .unwrap_or_else(|| crate::config::DEFAULT_REGION.to_string())
    }

    /// Force a token refresh (e.g., after a 403 response).
    pub async fn force_refresh(&self) -> Result<()> {
        info!("Force refresh requested");
        self.refresh().await
    }

    /// Attempt to load credentials from storage.
    pub async fn load_from_storage(&self) -> Result<bool> {
        if let Some(storage) = &self.storage {
            if let Some(token) = storage.load(&self.provider).await? {
                info!(source = storage.name(), "Loaded credentials from storage");
                let mut current = self.token.write().await;
                *current = Some(token);
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Refresh the token using the appropriate endpoint.
    async fn refresh(&self) -> Result<()> {
        let mut token = self.token.write().await;
        let info = token.as_ref().ok_or(Error::NotAuthenticated)?;

        // Double-check: another task may have refreshed while we waited for the lock
        if !info.needs_refresh() && !info.access_token.is_empty() {
            return Ok(());
        }

        match info.auth_type {
            AuthType::KiroDesktop => {
                debug!("Refreshing via Kiro Desktop Auth");
                let response =
                    super::kiro_desktop::refresh_token(&self.client, info, &self.fingerprint)
                        .await?;

                let mut updated = info.clone();
                updated.access_token = response.access_token;
                if let Some(new_refresh) = response.refresh_token {
                    if !new_refresh.is_empty() {
                        updated.refresh_token = new_refresh;
                    }
                }
                if let Some(arn) = response.profile_arn {
                    if !arn.is_empty() {
                        updated.profile_arn = Some(arn);
                    }
                }
                updated.expires_at =
                    chrono::Utc::now().timestamp() + response.expires_in;

                // Persist to storage
                if let Some(storage) = &self.storage {
                    if let Err(e) = storage.save(&self.provider, &updated).await {
                        warn!("Failed to persist token: {}", e);
                    }
                }

                *token = Some(updated);
            }
            AuthType::AwsSsoOidc => {
                debug!("Refreshing via AWS SSO OIDC");
                let response =
                    super::aws_sso_oidc::refresh_token(&self.client, info).await?;

                let mut updated = info.clone();
                updated.access_token = response.access_token;
                if let Some(new_refresh) = response.refresh_token {
                    if !new_refresh.is_empty() {
                        updated.refresh_token = new_refresh;
                    }
                }
                updated.expires_at =
                    chrono::Utc::now().timestamp() + response.expires_in;

                if let Some(storage) = &self.storage {
                    if let Err(e) = storage.save(&self.provider, &updated).await {
                        warn!("Failed to persist token: {}", e);
                    }
                }

                *token = Some(updated);
            }
        }

        info!("Token refreshed successfully");
        Ok(())
    }
}

impl std::fmt::Debug for KiroAuthManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KiroAuthManager")
            .field("fingerprint", &self.fingerprint)
            .field("provider", &self.provider)
            .field("has_storage", &self.storage.is_some())
            .finish()
    }
}
