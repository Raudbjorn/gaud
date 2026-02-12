//! OAuth orchestration module.
//!
//! Manages OAuth flows, state storage, and token exchange for various providers.

pub mod callback;
pub mod pkce;
pub mod claude;
pub mod copilot;
pub mod gemini;

use crate::auth::error::AuthError;
use crate::auth::store::TokenStorage;
use crate::auth::tokens::TokenInfo;
use crate::config::{Config, StorageBackend};
use crate::db::Database;
use std::sync::Arc;
use tracing::{debug, info, warn};

// Re-exports
pub use callback::{
    cleanup_expired_states, error_html, store_state_in_db, success_html, validate_callback_params,
    validate_state_from_db, CallbackParams, CallbackResult,
};
pub use pkce::Pkce;

// =============================================================================
// OAuthStatus
// =============================================================================

/// Status of OAuth authentication for a provider.
#[derive(Debug, Clone, serde::Serialize)]
pub struct OAuthStatus {
    /// Provider identifier.
    pub provider: String,
    /// Whether valid tokens exist.
    pub authenticated: bool,
    /// Whether the token is expired.
    pub expired: bool,
    /// Whether the token needs proactive refresh.
    pub needs_refresh: bool,
    /// Seconds until the token expires (None if no expiry or not authenticated).
    pub expires_in_secs: Option<u64>,
}

// =============================================================================
// OAuthManager
// =============================================================================

/// Central OAuth manager.
///
/// Orchestrates OAuth flows for all providers, manages state tokens
/// in SQLite, and delegates token storage to the configured backend.
pub struct OAuthManager {
    config: Arc<Config>,
    db: Database,
    storage: Arc<dyn TokenStorage>,
    http_client: reqwest::Client,
}

impl OAuthManager {
    /// Create a new OAuthManager.
    pub fn new(config: Arc<Config>, db: Database, storage: Arc<dyn TokenStorage>) -> Result<Self, AuthError> {
        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent("gaud/0.1.0") // Standardized User-Agent
            .build()
            .map_err(|e| AuthError::Other(format!("Failed to build HTTP client: {}", e)))?;

        Ok(Self {
            config,
            db,
            storage,
            http_client,
        })
    }

    /// Create an OAuthManager using the storage backend from config.
    pub fn from_config(config: Arc<Config>, db: Database) -> Result<Self, AuthError> {
        use crate::auth::store::{FileTokenStorage, MemoryTokenStorage};

        // Handle Keyring conditionally
        #[cfg(feature = "system-keyring")]
        use crate::auth::store::KeyringTokenStorage;

        let storage: Arc<dyn TokenStorage> = match config.providers.storage_backend {
            StorageBackend::File => {
                Arc::new(FileTokenStorage::new(&config.providers.token_storage_dir))
            }
            #[cfg(feature = "system-keyring")]
            StorageBackend::Keyring => Arc::new(KeyringTokenStorage::new()),
            #[cfg(not(feature = "system-keyring"))]
            StorageBackend::Keyring => {
                tracing::warn!(
                    "Keyring storage requested but system-keyring feature not enabled, falling back to file storage"
                );
                Arc::new(FileTokenStorage::new(&config.providers.token_storage_dir))
            }
            StorageBackend::Memory => Arc::new(MemoryTokenStorage::new()),
        };

        Self::new(config, db, storage)
    }

    /// Get a reference to the HTTP client.
    pub fn http_client(&self) -> &reqwest::Client {
        &self.http_client
    }

    /// Get a reference to the token storage.
    pub fn storage(&self) -> Arc<dyn TokenStorage> {
        self.storage.clone()
    }

    // =========================================================================
    // Flow: start_flow
    // =========================================================================

    /// Start an OAuth flow for the given provider.
    ///
    /// Note: Copilot uses a device code flow and should use `start_copilot_device_flow` instead.
    pub fn start_flow(&self, provider: &str) -> Result<String, AuthError> {
        match provider {
            "claude" => self.start_claude_flow(),
            "gemini" => self.start_gemini_flow(),
            "copilot" => Err(AuthError::Other(
                "Copilot uses device code flow; use start_copilot_device_flow()".to_string(),
            )),
            "kiro" => Err(AuthError::Other(
                "Kiro uses internal auth (refresh token / AWS SSO); no browser OAuth flow required"
                    .to_string(),
            )),
            _ => Err(AuthError::Other(format!("Unknown provider: {}", provider))),
        }
    }

    fn start_claude_flow(&self) -> Result<String, AuthError> {
        let provider_config = self
            .config
            .providers
            .claude
            .as_ref()
            .ok_or_else(|| AuthError::Other("Claude provider not configured".to_string()))?;

        // Note: Using existing provider structs. Might need refactoring later if we move them.
        let oauth_config = claude::ClaudeOAuthConfig::from_provider_config(
            &provider_config.client_id,
            &provider_config.auth_url,
            provider_config.callback_port,
        );

        let pkce = Pkce::generate();
        let state = uuid::Uuid::new_v4().to_string();

        // Store state in DB. Note: store_state_in_db returns AuthError now (via conversion)
        store_state_in_db(&self.db, &state, "claude", &pkce.verifier)?;

        let url = claude::build_authorize_url(&oauth_config, &pkce, &state);
        info!(provider = "claude", "Started OAuth flow");
        Ok(url)
    }

    fn start_gemini_flow(&self) -> Result<String, AuthError> {
        let provider_config = self
            .config
            .providers
            .gemini
            .as_ref()
            .ok_or_else(|| AuthError::Other("Gemini provider not configured".to_string()))?;

        let oauth_config = gemini::GeminiOAuthConfig::from_provider_config(
            &provider_config.client_id,
            &provider_config.client_secret,
            &provider_config.auth_url,
            &provider_config.token_url,
            provider_config.callback_port,
        );

        let pkce = Pkce::generate();
        let state = uuid::Uuid::new_v4().to_string();

        store_state_in_db(&self.db, &state, "gemini", &pkce.verifier)?;

        let url = gemini::build_authorize_url(&oauth_config, &pkce, &state);
        info!(provider = "gemini", "Started OAuth flow");
        Ok(url)
    }

    /// Start the Copilot device code flow.
    pub async fn start_copilot_device_flow(
        &self,
    ) -> Result<copilot::DeviceCodeResponse, AuthError> {
        let provider_config = self
            .config
            .providers
            .copilot
            .as_ref()
            .ok_or_else(|| AuthError::Other("Copilot provider not configured".to_string()))?;

        let oauth_config =
            copilot::CopilotOAuthConfig::from_provider_config(&provider_config.client_id);

        // Map provider-specific error to AuthError
        copilot::request_device_code(&self.http_client, &oauth_config)
            .await
            .map_err(|e| AuthError::Other(e.to_string()))
    }

    // =========================================================================
    // Flow: complete_flow
    // =========================================================================

    /// Complete an OAuth flow by exchanging the authorization code for tokens.
    pub async fn complete_flow(
        &self,
        provider: &str,
        code: &str,
        state: &str,
    ) -> Result<TokenInfo, AuthError> {
        // Validate state and get verifier from DB
        let (db_provider, code_verifier) = validate_state_from_db(&self.db, state)?;

        // Verify provider matches
        if db_provider != provider {
            warn!(
                expected = %provider,
                actual = %db_provider,
                "Provider mismatch in OAuth callback"
            );
            return Err(AuthError::InvalidState);
        }

        let token = match provider {
            "claude" => self.complete_claude_flow(code, &code_verifier).await?,
            "gemini" => self.complete_gemini_flow(code, &code_verifier).await?,
            "copilot" => {
                 return Err(AuthError::Other(
                    "Copilot uses device code flow; use complete_copilot_device_flow()".to_string(),
                ));
            }
            _ => {
                return Err(AuthError::Other(format!(
                    "Cannot complete flow for provider: {}",
                    provider
                )));
            }
        };

        // Store the token
        self.storage.save(provider, &token)?;
        info!(provider, "OAuth flow completed, token stored");

        Ok(token)
    }

    async fn complete_claude_flow(
        &self,
        code: &str,
        verifier: &str,
    ) -> Result<TokenInfo, AuthError> {
        let provider_config = self
            .config
            .providers
            .claude
            .as_ref()
            .ok_or_else(|| AuthError::Other("Claude provider not configured".to_string()))?;

        let oauth_config = claude::ClaudeOAuthConfig::from_provider_config(
            &provider_config.client_id,
            &provider_config.auth_url,
            provider_config.callback_port,
        );

        claude::exchange_code(&self.http_client, &oauth_config, code, verifier)
            .await
            .map(TokenInfo::from)
            .map_err(|e| AuthError::ExchangeFailed(e.to_string()))
    }

    async fn complete_gemini_flow(
        &self,
        code: &str,
        verifier: &str,
    ) -> Result<TokenInfo, AuthError> {
        let provider_config = self
            .config
            .providers
            .gemini
            .as_ref()
            .ok_or_else(|| AuthError::Other("Gemini provider not configured".to_string()))?;

        let oauth_config = gemini::GeminiOAuthConfig::from_provider_config(
            &provider_config.client_id,
            &provider_config.client_secret,
            &provider_config.auth_url,
            &provider_config.token_url,
            provider_config.callback_port,
        );

        gemini::exchange_code(&self.http_client, &oauth_config, code, verifier)
            .await
            .map(TokenInfo::from)
            .map_err(|e| AuthError::ExchangeFailed(e.to_string()))
    }

    // =========================================================================
    // Token management
    // =========================================================================

    pub async fn refresh_token(&self, provider: &str) -> Result<TokenInfo, AuthError> {
        let current = self
            .storage
            .load(provider)?
            .ok_or_else(|| AuthError::TokenNotFound(provider.to_string()))?;

        let refresh = current.refresh_token.as_deref().ok_or_else(|| {
            AuthError::ExchangeFailed(format!("No refresh token for {}", provider))
        })?;

        let new_token = match provider {
            "claude" => {
                let pc = self.config.providers.claude.as_ref().ok_or_else(|| {
                    AuthError::Other("Claude provider not configured".to_string())
                })?;
                let oc = claude::ClaudeOAuthConfig::from_provider_config(
                    &pc.client_id,
                    &pc.auth_url,
                    pc.callback_port,
                );

                claude::refresh_token(&self.http_client, &oc, refresh)
                    .await
                    .map(TokenInfo::from)
                    .map_err(|e| AuthError::ExchangeFailed(e.to_string()))?
            }
            "gemini" => {
                let pc = self.config.providers.gemini.as_ref().ok_or_else(|| {
                    AuthError::Other("Gemini provider not configured".to_string())
                })?;
                let oc = gemini::GeminiOAuthConfig::from_provider_config(
                    &pc.client_id,
                    &pc.client_secret,
                    &pc.auth_url,
                    &pc.token_url,
                    pc.callback_port,
                );

                gemini::refresh_token(&self.http_client, &oc, refresh)
                    .await
                    .map(TokenInfo::from)
                    .map_err(|e| AuthError::ExchangeFailed(e.to_string()))?
            }
            "copilot" => {
                return Err(AuthError::ExchangeFailed(
                    "Copilot tokens don't support refresh; re-authenticate via device flow"
                        .to_string(),
                ));
            }
            _ => {
                return Err(AuthError::Other(format!("Unknown provider: {}", provider)));
            }
        };

        self.storage.save(provider, &new_token)?;
        debug!(provider, "Token refreshed successfully");
        Ok(new_token)
    }

    pub fn get_status(&self, provider: &str) -> Result<OAuthStatus, AuthError> {
        // Kiro special case
        if provider == "kiro" {
            let kiro_config = self.config.providers.kiro.as_ref();
            let configured = kiro_config.is_some();
            let has_credentials = kiro_config.map(|c| c.has_credentials()).unwrap_or(false);
            return Ok(OAuthStatus {
                provider: provider.to_string(),
                authenticated: configured && has_credentials,
                expired: false,
                needs_refresh: false,
                expires_in_secs: None,
            });
        }

        let token = self.storage.load(provider)?;

        match token {
            Some(t) => Ok(OAuthStatus {
                provider: provider.to_string(),
                authenticated: true,
                expired: t.is_expired(),
                needs_refresh: t.needs_refresh(),
                expires_in_secs: t.expires_at.map(|exp| {
                    let now = chrono::Utc::now().timestamp();
                    if exp > now { (exp - now) as u64 } else { 0 }
                }),
            }),
            None => Ok(OAuthStatus {
                provider: provider.to_string(),
                authenticated: false,
                expired: false,
                needs_refresh: false,
                expires_in_secs: None,
            }),
        }
    }

    pub fn get_token(&self, provider: &str) -> Result<Option<TokenInfo>, AuthError> {
        self.storage.load(provider)
    }

    pub async fn get_valid_token(&self, provider: &str) -> Result<String, AuthError> {
        let token = self
            .storage
            .load(provider)?
            .ok_or_else(|| AuthError::TokenNotFound(provider.to_string()))?;

        if token.needs_refresh() {
            debug!(provider, "Token needs refresh, refreshing...");
            let new_token = self.refresh_token(provider).await?;
            return Ok(new_token.access_token);
        }

        Ok(token.access_token)
    }

    pub fn remove_token(&self, provider: &str) -> Result<(), AuthError> {
        self.storage.remove(provider)?;
        info!(provider, "Token removed");
        Ok(())
    }
}

use crate::auth::traits::TokenProvider;

#[async_trait::async_trait]
impl TokenProvider for OAuthManager {
    async fn get_token(&self, provider: &str) -> Result<String, AuthError> {
        self.get_valid_token(provider).await
    }
}
