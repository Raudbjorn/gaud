//! OAuth module for gaud multi-user LLM proxy.
//!
//! Provides OAuth 2.0 authentication for multiple LLM providers:
//! - Claude (Anthropic) - PKCE authorization code flow
//! - Gemini (Google) - PKCE authorization code flow with client secret
//! - Copilot (GitHub) - Device Code flow (RFC 8628)
//!
//! # Architecture
//!
//! - [`OAuthManager`] - Central manager that orchestrates flows, stores state in SQLite
//! - [`TokenStorage`] - Pluggable token persistence (file, keyring, memory)
//! - [`TokenInfo`] - Token data with composite format and expiry checking
//! - [`Pkce`] - PKCE S256 challenge/verifier generation
//! - Provider modules (`claude`, `gemini`, `copilot`) - Provider-specific flows
//! - [`callback`] - OAuth callback handler with HTML responses
//!
//! # Example
//!
//! ```rust,ignore
//! use gaud::oauth::{OAuthManager, OAuthError};
//! use gaud::oauth::storage::FileTokenStorage;
//!
//! let storage = FileTokenStorage::new("/var/lib/gaud/tokens");
//! let manager = OAuthManager::new(config, db, Box::new(storage));
//!
//! // Start a Claude OAuth flow
//! let auth_url = manager.start_flow("claude")?;
//! // ... user visits URL, callback fires ...
//! // manager.complete_flow("claude", code, state).await?;
//! ```

pub mod callback;
pub mod claude;
pub mod copilot;
pub mod gemini;
pub mod pkce;
pub mod storage;
pub mod token;

// Re-exports
pub use callback::{
    CallbackParams, CallbackResult, cleanup_expired_states, error_html, store_state_in_db,
    success_html, validate_callback_params, validate_state_from_db,
};
pub use pkce::Pkce;
pub use storage::{FileTokenStorage, MemoryTokenStorage, TokenStorage};
pub use token::TokenInfo;

#[cfg(feature = "system-keyring")]
pub use storage::KeyringTokenStorage;

use std::sync::Arc;

use oauth2::basic::BasicErrorResponseType;
use tracing::{debug, info, warn};

use crate::config::{Config, StorageBackend};
use crate::db::Database;
use crate::providers::{ProviderError, TokenService};

// =============================================================================
// TokenProvider Trait
// =============================================================================

/// Trait for providing access tokens.
///
/// This abstracts the source of tokens (e.g., OAuthManager, static token)
/// from the consumers (e.g., API clients).
#[async_trait::async_trait]
pub trait TokenProvider: Send + Sync {
    /// Get a valid access token for the specified provider.
    ///
    /// The implementation should handle refreshing if necessary.
    async fn get_token(&self, provider: &str) -> Result<String, OAuthError>;
}

impl From<OAuthError> for ProviderError {
    fn from(err: OAuthError) -> Self {
        match err {
            OAuthError::TokenNotFound(p) => ProviderError::NoToken { provider: p },
            OAuthError::TokenExpired(p) => ProviderError::NoToken { provider: p },
            OAuthError::Http(e) => ProviderError::Http(e),
            other => ProviderError::Other(other.to_string()),
        }
    }
}

// =============================================================================
// OAuthError
// =============================================================================

/// Errors that can occur during OAuth operations.
#[derive(Debug, thiserror::Error)]
pub enum OAuthError {
    /// Token not found for the given provider.
    #[error("Token not found for {0}")]
    TokenNotFound(String),

    /// Token has expired and needs re-authentication.
    #[error("Token expired for {0}")]
    TokenExpired(String),

    /// Token exchange or refresh failed.
    #[error("Exchange failed: {0}")]
    ExchangeFailed(String),

    /// Token storage error.
    #[error("Storage error: {0}")]
    Storage(String),

    /// Invalid or unknown state token (possible CSRF).
    #[error("Invalid state token")]
    InvalidState,

    /// The OAuth flow has expired (e.g., device code timeout).
    #[error("Flow expired")]
    FlowExpired,

    /// HTTP client error.
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    /// Generic error.
    #[error("{0}")]
    Other(String),
}

// =============================================================================
// Shared OAuth2 Helpers
// =============================================================================

/// Fully-configured `BasicClient` with auth and token endpoints set.
///
/// Used by all `oauth2`-based providers (Claude, Gemini).
pub(crate) type OAuthClient = oauth2::basic::BasicClient<
    oauth2::EndpointSet,
    oauth2::EndpointNotSet,
    oauth2::EndpointNotSet,
    oauth2::EndpointNotSet,
    oauth2::EndpointSet,
>;

/// Map an `oauth2::RequestTokenError` to [`OAuthError`].
///
/// Shared across all `oauth2`-based providers. Preserves the
/// `invalid_grant` → `TokenExpired` mapping used for refresh retry logic.
pub(crate) fn map_oauth_token_error<RE: std::error::Error + 'static>(
    provider: &str,
    err: oauth2::RequestTokenError<RE, oauth2::StandardErrorResponse<BasicErrorResponseType>>,
) -> OAuthError {
    match err {
        oauth2::RequestTokenError::ServerResponse(ref server_err) => {
            let error_type = server_err.error();
            let description = server_err
                .error_description()
                .cloned()
                .unwrap_or_else(|| error_type.to_string());

            warn!(
                %provider,
                error = %error_type,
                description = %description,
                "Token request failed"
            );

            if *error_type == BasicErrorResponseType::InvalidGrant {
                return OAuthError::TokenExpired(provider.to_string());
            }

            OAuthError::ExchangeFailed(description)
        }
        oauth2::RequestTokenError::Request(ref req_err) => {
            OAuthError::ExchangeFailed(format!("HTTP request failed: {}", req_err))
        }
        oauth2::RequestTokenError::Parse(ref parse_err, _) => {
            OAuthError::ExchangeFailed(format!("Failed to parse token response: {}", parse_err))
        }
        oauth2::RequestTokenError::Other(msg) => OAuthError::ExchangeFailed(msg),
    }
}

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
    pub fn new(config: Arc<Config>, db: Database, storage: Arc<dyn TokenStorage>) -> Self {
        let http_client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_default();

        Self {
            config,
            db,
            storage,
            http_client,
        }
    }

    /// Create an OAuthManager using the storage backend from config.
    pub fn from_config(config: Arc<Config>, db: Database) -> Self {
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
    /// Generates PKCE, stores the state token in the database, and returns
    /// the authorization URL the user should visit.
    ///
    /// For Copilot (device code flow), this returns the GitHub verification
    /// URL. The caller should also use `start_copilot_device_flow()` for
    /// the full device code response.
    pub fn start_flow(&self, provider: &str) -> Result<String, OAuthError> {
        match provider {
            "claude" => self.start_claude_flow(),
            "gemini" => self.start_gemini_flow(),
            "copilot" => Ok("https://github.com/login/device".to_string()),
            "kiro" => Err(OAuthError::Other(
                "Kiro uses internal auth (refresh token / AWS SSO); no browser OAuth flow required"
                    .to_string(),
            )),
            _ => Err(OAuthError::Other(format!("Unknown provider: {}", provider))),
        }
    }

    fn start_claude_flow(&self) -> Result<String, OAuthError> {
        let provider_config = self
            .config
            .providers
            .claude
            .as_ref()
            .ok_or_else(|| OAuthError::Other("Claude provider not configured".to_string()))?;

        let oauth_config = claude::ClaudeOAuthConfig::from_provider_config(
            &provider_config.client_id,
            &provider_config.auth_url,
            provider_config.callback_port,
        );

        let state = uuid::Uuid::new_v4().to_string();
        let (url, verifier) = claude::build_authorize_url(&oauth_config, &state)?;

        // Store state in DB
        store_state_in_db(&self.db, &state, "claude", &verifier)?;

        info!(provider = "claude", "Started OAuth flow");
        Ok(url)
    }

    fn start_gemini_flow(&self) -> Result<String, OAuthError> {
        let provider_config = self
            .config
            .providers
            .gemini
            .as_ref()
            .ok_or_else(|| OAuthError::Other("Gemini provider not configured".to_string()))?;

        let oauth_config = gemini::GeminiOAuthConfig::from_provider_config(
            &provider_config.client_id,
            &provider_config.client_secret,
            &provider_config.auth_url,
            &provider_config.token_url,
            provider_config.callback_port,
        );

        let state = uuid::Uuid::new_v4().to_string();
        let (url, verifier) = gemini::build_authorize_url(&oauth_config, &state)?;

        store_state_in_db(&self.db, &state, "gemini", &verifier)?;

        info!(provider = "gemini", "Started OAuth flow");
        Ok(url)
    }

    /// Start the Copilot device code flow.
    ///
    /// Returns the device code response containing the user_code and
    /// verification_uri that should be displayed to the user.
    pub async fn start_copilot_device_flow(
        &self,
    ) -> Result<copilot::DeviceCodeResponse, OAuthError> {
        let provider_config = self
            .config
            .providers
            .copilot
            .as_ref()
            .ok_or_else(|| OAuthError::Other("Copilot provider not configured".to_string()))?;

        let oauth_config =
            copilot::CopilotOAuthConfig::from_provider_config(&provider_config.client_id);

        copilot::request_device_code(&self.http_client, &oauth_config).await
    }

    // =========================================================================
    // Flow: complete_flow
    // =========================================================================

    /// Complete an OAuth flow by exchanging the authorization code for tokens.
    ///
    /// Validates the state token against the database, exchanges the code
    /// using the stored PKCE verifier, and saves the resulting tokens.
    pub async fn complete_flow(
        &self,
        provider: &str,
        code: &str,
        state: &str,
    ) -> Result<TokenInfo, OAuthError> {
        // Validate state and get verifier from DB
        let (db_provider, code_verifier) = validate_state_from_db(&self.db, state)?;

        // Verify provider matches
        if db_provider != provider {
            warn!(
                expected = %provider,
                actual = %db_provider,
                "Provider mismatch in OAuth callback"
            );
            return Err(OAuthError::InvalidState);
        }

        let token = match provider {
            "claude" => self.complete_claude_flow(code, &code_verifier).await?,
            "gemini" => self.complete_gemini_flow(code, &code_verifier).await?,
            _ => {
                return Err(OAuthError::Other(format!(
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
    ) -> Result<TokenInfo, OAuthError> {
        let provider_config = self
            .config
            .providers
            .claude
            .as_ref()
            .ok_or_else(|| OAuthError::Other("Claude provider not configured".to_string()))?;

        let oauth_config = claude::ClaudeOAuthConfig::from_provider_config(
            &provider_config.client_id,
            &provider_config.auth_url,
            provider_config.callback_port,
        );

        claude::exchange_code(&self.http_client, &oauth_config, code, verifier).await
    }

    async fn complete_gemini_flow(
        &self,
        code: &str,
        verifier: &str,
    ) -> Result<TokenInfo, OAuthError> {
        let provider_config = self
            .config
            .providers
            .gemini
            .as_ref()
            .ok_or_else(|| OAuthError::Other("Gemini provider not configured".to_string()))?;

        let oauth_config = gemini::GeminiOAuthConfig::from_provider_config(
            &provider_config.client_id,
            &provider_config.client_secret,
            &provider_config.auth_url,
            &provider_config.token_url,
            provider_config.callback_port,
        );

        gemini::exchange_code(&self.http_client, &oauth_config, code, verifier).await
    }

    /// Complete the Copilot device code flow by polling until authorized.
    ///
    /// Polls GitHub until the user completes authorization, then stores
    /// the resulting token.
    pub async fn complete_copilot_flow(
        &self,
        device_response: &copilot::DeviceCodeResponse,
        on_pending: Option<&mut dyn FnMut(u32)>,
    ) -> Result<TokenInfo, OAuthError> {
        let provider_config = self
            .config
            .providers
            .copilot
            .as_ref()
            .ok_or_else(|| OAuthError::Other("Copilot provider not configured".to_string()))?;

        let oauth_config =
            copilot::CopilotOAuthConfig::from_provider_config(&provider_config.client_id);

        let access_token = copilot::poll_until_complete(
            &self.http_client,
            &oauth_config,
            device_response,
            on_pending,
        )
        .await?;

        let token = copilot::create_token_info(&access_token);
        self.storage.save("copilot", &token)?;
        info!(
            provider = "copilot",
            "Device code flow completed, token stored"
        );

        Ok(token)
    }

    // =========================================================================
    // Token management
    // =========================================================================

    /// Refresh the token for a provider.
    ///
    /// Loads the current token, uses its refresh token to obtain a new
    /// access token, and saves the updated token.
    pub async fn refresh_token(&self, provider: &str) -> Result<TokenInfo, OAuthError> {
        let current = self
            .storage
            .load(provider)?
            .ok_or_else(|| OAuthError::TokenNotFound(provider.to_string()))?;

        let refresh = current.refresh_token.as_deref().ok_or_else(|| {
            OAuthError::ExchangeFailed(format!("No refresh token for {}", provider))
        })?;

        let new_token = match provider {
            "claude" => {
                let pc = self.config.providers.claude.as_ref().ok_or_else(|| {
                    OAuthError::Other("Claude provider not configured".to_string())
                })?;
                let oc = claude::ClaudeOAuthConfig::from_provider_config(
                    &pc.client_id,
                    &pc.auth_url,
                    pc.callback_port,
                );
                claude::refresh_token(&self.http_client, &oc, refresh).await?
            }
            "gemini" => {
                let pc = self.config.providers.gemini.as_ref().ok_or_else(|| {
                    OAuthError::Other("Gemini provider not configured".to_string())
                })?;
                let oc = gemini::GeminiOAuthConfig::from_provider_config(
                    &pc.client_id,
                    &pc.client_secret,
                    &pc.auth_url,
                    &pc.token_url,
                    pc.callback_port,
                );
                gemini::refresh_token(&self.http_client, &oc, refresh).await?
            }
            "copilot" => {
                // Copilot doesn't use refresh tokens in the traditional sense;
                // the GitHub token itself is long-lived
                return Err(OAuthError::ExchangeFailed(
                    "Copilot tokens don't support refresh; re-authenticate via device flow"
                        .to_string(),
                ));
            }
            _ => {
                return Err(OAuthError::Other(format!("Unknown provider: {}", provider)));
            }
        };

        self.storage.save(provider, &new_token)?;
        debug!(provider, "Token refreshed successfully");
        Ok(new_token)
    }

    /// Get the current OAuth status for a provider.
    ///
    /// For Kiro, auth is managed internally by the kiro-gateway client.
    /// We report `authenticated: true` only if the provider is configured
    /// AND credentials are available. The actual token validity is checked
    /// at request time by kiro-gateway.
    pub fn get_status(&self, provider: &str) -> Result<OAuthStatus, OAuthError> {
        // Kiro manages its own auth -- check if configured and has credentials.
        if provider == "kiro" {
            let kiro_config = self.config.providers.kiro.as_ref();
            let configured = kiro_config.is_some();
            // Report as authenticated only if configured and credential source
            // is specified (refresh token, credentials file, or SQLite DB).
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

    /// Get the stored token for a provider, if any.
    pub fn get_token(&self, provider: &str) -> Result<Option<TokenInfo>, OAuthError> {
        self.storage.load(provider)
    }

    /// Get a valid access token for a provider, refreshing if needed.
    ///
    /// Returns the access token string ready for use in API requests.
    pub async fn get_valid_token(&self, provider: &str) -> Result<String, OAuthError> {
        let token = self
            .storage
            .load(provider)?
            .ok_or_else(|| OAuthError::TokenNotFound(provider.to_string()))?;

        if token.needs_refresh() {
            debug!(provider, "Token needs refresh, refreshing...");
            let new_token = self.refresh_token(provider).await?;
            return Ok(new_token.access_token);
        }

        Ok(token.access_token)
    }

    /// Remove stored tokens for a provider (logout).
    pub fn remove_token(&self, provider: &str) -> Result<(), OAuthError> {
        self.storage.remove(provider)?;
        info!(provider, "Token removed");
        Ok(())
    }
}

#[async_trait::async_trait]
impl TokenService for OAuthManager {
    async fn get_token(&self, provider: &str) -> Result<String, ProviderError> {
        self.get_valid_token(provider).await.map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl TokenProvider for OAuthManager {
    async fn get_token(&self, provider: &str) -> Result<String, OAuthError> {
        self.get_valid_token(provider).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> Config {
        Config::default()
    }

    fn test_db() -> Database {
        Database::open_in_memory().unwrap()
    }

    #[test]
    fn test_oauth_manager_creation() {
        let config = Arc::new(test_config());
        let db = test_db();
        let storage = Arc::new(MemoryTokenStorage::new());
        let manager = OAuthManager::new(config, db, storage);
        assert_eq!(manager.storage().name(), "memory");
    }

    #[test]
    fn test_oauth_manager_from_config() {
        let config = Arc::new(test_config());
        let db = test_db();
        let manager = OAuthManager::from_config(config, db);
        // Default storage backend is file
        assert_eq!(manager.storage().name(), "file");
    }

    #[test]
    fn test_get_status_unauthenticated() {
        let config = Arc::new(test_config());
        let db = test_db();
        let storage = Arc::new(MemoryTokenStorage::new());
        let manager = OAuthManager::new(config, db, storage);

        let status = manager.get_status("claude").unwrap();
        assert_eq!(status.provider, "claude");
        assert!(!status.authenticated);
        assert!(!status.expired);
        assert!(!status.needs_refresh);
        assert!(status.expires_in_secs.is_none());
    }

    #[test]
    fn test_get_status_authenticated() {
        let config = Arc::new(test_config());
        let db = test_db();
        let storage = Box::new(MemoryTokenStorage::new());

        let token = TokenInfo::new(
            "access_token".into(),
            Some("refresh_token".into()),
            Some(3600),
            "claude",
        );
        storage.save("claude", &token).unwrap();

        let manager = OAuthManager::new(config, db, Arc::new(storage));
        let status = manager.get_status("claude").unwrap();
        assert!(status.authenticated);
        assert!(!status.expired);
        assert!(!status.needs_refresh);
        assert!(status.expires_in_secs.is_some());
    }

    #[test]
    fn test_get_token() {
        let config = Arc::new(test_config());
        let db = test_db();
        let storage = Box::new(MemoryTokenStorage::new());

        let token = TokenInfo::new(
            "access_token".into(),
            Some("refresh_token".into()),
            Some(3600),
            "claude",
        );
        storage.save("claude", &token).unwrap();

        let manager = OAuthManager::new(config, db, Arc::new(storage));
        let loaded = manager.get_token("claude").unwrap();
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().access_token, "access_token");
    }

    #[test]
    fn test_get_token_missing() {
        let config = Arc::new(test_config());
        let db = test_db();
        let storage = Arc::new(MemoryTokenStorage::new());
        let manager = OAuthManager::new(config, db, storage);

        let loaded = manager.get_token("claude").unwrap();
        assert!(loaded.is_none());
    }

    #[test]
    fn test_remove_token() {
        let config = Arc::new(test_config());
        let db = test_db();
        let storage = Box::new(MemoryTokenStorage::new());

        let token = TokenInfo::new(
            "access_token".into(),
            Some("refresh_token".into()),
            Some(3600),
            "claude",
        );
        storage.save("claude", &token).unwrap();

        let manager = OAuthManager::new(config, db, Arc::new(storage));
        manager.remove_token("claude").unwrap();
        assert!(manager.get_token("claude").unwrap().is_none());
    }

    #[test]
    fn test_start_flow_unknown_provider() {
        let config = Arc::new(test_config());
        let db = test_db();
        let storage = Arc::new(MemoryTokenStorage::new());
        let manager = OAuthManager::new(config, db, storage);

        let result = manager.start_flow("unknown");
        assert!(result.is_err());
    }

    #[test]
    fn test_start_flow_unconfigured_claude() {
        let config = Arc::new(test_config()); // No Claude config
        let db = test_db();
        let storage = Arc::new(MemoryTokenStorage::new());
        let manager = OAuthManager::new(config, db, storage);

        let result = manager.start_flow("claude");
        assert!(result.is_err());
    }

    #[test]
    fn test_start_flow_copilot_returns_github_url() {
        let config = Arc::new(test_config());
        let db = test_db();
        let storage = Arc::new(MemoryTokenStorage::new());
        let manager = OAuthManager::new(config, db, storage);

        let url = manager.start_flow("copilot").unwrap();
        assert!(url.contains("github.com"));
    }

    #[test]
    fn test_oauth_error_display() {
        let err = OAuthError::TokenNotFound("claude".to_string());
        assert_eq!(err.to_string(), "Token not found for claude");

        let err = OAuthError::InvalidState;
        assert_eq!(err.to_string(), "Invalid state token");

        let err = OAuthError::FlowExpired;
        assert_eq!(err.to_string(), "Flow expired");
    }
}
