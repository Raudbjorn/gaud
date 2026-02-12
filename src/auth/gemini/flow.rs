//! OAuth flow orchestrator.
//!
//! This module provides the [`OAuthFlow`] struct which orchestrates the complete
//! OAuth authentication lifecycle including:
//!
//! - Starting authorization (generating PKCE and authorization URL)
//! - Exchanging authorization codes for tokens
//! - Refreshing access tokens automatically
//! - Token storage and retrieval
//! - Logout functionality
//!
//! # Example
//!
//! ```rust,ignore
//! use gaud::gemini::OAuthFlow;
//! use gaud::gemini::storage::MemoryTokenStorage;
//!
//! # async fn example() -> gaud::gemini::Result<()> {
//! let storage = MemoryTokenStorage::new();
//! let mut flow = OAuthFlow::new(storage);
//!
//! // Check if already authenticated
//! if !flow.is_authenticated().await? {
//!     // Start OAuth flow
//!     let (url, state) = flow.start_authorization()?;
//!     println!("Open: {}", url);
//!
//!     // After user authorizes, exchange the code
//!     // let code = "..."; // From callback
//!     // flow.exchange_code(code, Some(&state.state)).await?;
//! }
//!
//! // Get access token (auto-refreshes if needed)
//! let token = flow.get_access_token().await?;
//! # Ok(())
//! # }
//! ```

use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, instrument, warn};

use crate::auth::gemini::oauth::{self, OAuthFlowState};
use crate::auth::gemini::TokenInfo;
use crate::gemini::constants::{OAuthConfig, DEFAULT_OAUTH_CONFIG};
use crate::gemini::error::{AuthError, Error, Result};
use crate::gemini::storage::TokenStorage;

/// OAuth flow orchestrator.
///
/// Manages the complete OAuth lifecycle including authorization,
/// token exchange, refresh, and storage. The flow is generic over
/// the storage backend to support different persistence strategies.
///
/// # Thread Safety
///
/// `OAuthFlow` is `Send + Sync` when the storage backend is `Send + Sync`,
/// allowing use from multiple async tasks.
///
/// # Example
///
/// ```rust,ignore
/// use gaud::gemini::OAuthFlow;
/// use gaud::gemini::storage::FileTokenStorage;
///
/// # async fn example() -> gaud::gemini::Result<()> {
/// let storage = FileTokenStorage::default_path()?;
/// let flow = OAuthFlow::new(storage);
///
/// // Use from multiple tasks
/// let flow = std::sync::Arc::new(flow);
/// let flow_clone = flow.clone();
///
/// tokio::spawn(async move {
///     let token = flow_clone.get_access_token().await;
/// });
/// # Ok(())
/// # }
/// ```
pub struct OAuthFlow<S: TokenStorage> {
    /// Token storage backend.
    storage: S,
    /// OAuth configuration.
    config: OAuthConfig,
    /// Pending OAuth flow state (PKCE verifier, challenge, state).
    ///
    /// This is set when `start_authorization()` is called and cleared
    /// after `exchange_code()` completes.
    pending_state: Arc<RwLock<Option<OAuthFlowState>>>,
}

impl<S: TokenStorage> OAuthFlow<S> {
    /// Create a new OAuthFlow with the default OAuth configuration.
    ///
    /// # Arguments
    ///
    /// * `storage` - Token storage backend for persisting credentials
    ///
    /// # Example
    ///
    /// ```
    /// use gaud::gemini::OAuthFlow;
    /// use gaud::gemini::storage::MemoryTokenStorage;
    ///
    /// let storage = MemoryTokenStorage::new();
    /// let flow = OAuthFlow::new(storage);
    /// ```
    pub fn new(storage: S) -> Self {
        Self::with_config(storage, DEFAULT_OAUTH_CONFIG)
    }

    /// Create a new OAuthFlow with a custom OAuth configuration.
    ///
    /// # Arguments
    ///
    /// * `storage` - Token storage backend
    /// * `config` - Custom OAuth configuration
    ///
    /// # Example
    ///
    /// ```
    /// use gaud::gemini::OAuthFlow;
    /// use gaud::gemini::storage::MemoryTokenStorage;
    /// use gaud::gemini::constants::OAuthConfig;
    ///
    /// let config = OAuthConfig {
    ///     client_id: "my-client-id",
    ///     client_secret: "my-secret",
    ///     auth_url: "https://example.com/oauth/authorize",
    ///     token_url: "https://example.com/oauth/token",
    ///     user_info_url: "https://example.com/userinfo",
    ///     callback_port: 8080,
    ///     scopes: &["openid", "profile"],
    /// };
    ///
    /// let storage = MemoryTokenStorage::new();
    /// let flow = OAuthFlow::with_config(storage, config);
    /// ```
    pub fn with_config(storage: S, config: OAuthConfig) -> Self {
        Self {
            storage,
            config,
            pending_state: Arc::new(RwLock::new(None)),
        }
    }

    /// Start a new authorization flow.
    ///
    /// Generates PKCE verifier/challenge and state, then returns the
    /// authorization URL for the user to visit along with the flow state.
    ///
    /// The flow state should be stored temporarily and the state parameter
    /// should be validated when the callback is received.
    ///
    /// # Returns
    ///
    /// A tuple of `(authorization_url, flow_state)` where:
    /// - `authorization_url`: URL for user to visit to authorize
    /// - `flow_state`: Contains verifier and state for later validation
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use gaud::gemini::OAuthFlow;
    /// use gaud::gemini::storage::MemoryTokenStorage;
    ///
    /// // Note: Must be called from within a tokio runtime
    /// let storage = MemoryTokenStorage::new();
    /// let mut flow = OAuthFlow::new(storage);
    ///
    /// let (url, state) = flow.start_authorization().unwrap();
    /// println!("Open in browser: {}", url);
    /// println!("State for validation: {}", state.state);
    /// ```
    #[instrument(skip(self))]
    pub fn start_authorization(&self) -> Result<(String, OAuthFlowState)> {
        let flow_state = OAuthFlowState::new();

        let url = oauth::build_authorization_url(
            &self.config,
            &flow_state.code_challenge,
            &flow_state.state,
        );

        debug!(state = %flow_state.state, "Started OAuth authorization flow");

        // Store the pending state
        // Note: We use blocking lock here since this is a sync method
        // In practice, this should be fine as it's only called once at flow start
        let pending_clone = flow_state.clone();

        // Use try_write to avoid blocking, or spawn a task
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                let mut pending = self.pending_state.write().await;
                *pending = Some(pending_clone);
            });
        });

        Ok((url, flow_state))
    }

    /// Start authorization without blocking.
    ///
    /// Async version of `start_authorization()` that doesn't use block_in_place.
    /// Preferred when calling from an async context.
    #[instrument(skip(self))]
    pub async fn start_authorization_async(&self) -> Result<(String, OAuthFlowState)> {
        let flow_state = OAuthFlowState::new();

        let url = oauth::build_authorization_url(
            &self.config,
            &flow_state.code_challenge,
            &flow_state.state,
        );

        debug!(state = %flow_state.state, "Started OAuth authorization flow");

        // Store the pending state
        {
            let mut pending = self.pending_state.write().await;
            *pending = Some(flow_state.clone());
        }

        Ok((url, flow_state))
    }

    /// Exchange an authorization code for tokens.
    ///
    /// Completes the OAuth flow by exchanging the authorization code
    /// for access and refresh tokens. Optionally validates the state
    /// parameter to protect against CSRF attacks.
    ///
    /// # Arguments
    ///
    /// * `code` - Authorization code from the OAuth callback
    /// * `state` - Optional state parameter to validate (recommended)
    ///
    /// # State Validation
    ///
    /// If `state` is provided, it is validated against the pending flow state.
    /// This protects against CSRF attacks where an attacker might try to
    /// inject their own authorization code.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - State validation fails (`AuthError::StateMismatch`)
    /// - No pending flow state exists
    /// - Token exchange fails
    /// - Storage fails
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use gaud::gemini::OAuthFlow;
    /// use gaud::gemini::storage::MemoryTokenStorage;
    ///
    /// # async fn example() -> gaud::gemini::Result<()> {
    /// let storage = MemoryTokenStorage::new();
    /// let mut flow = OAuthFlow::new(storage);
    ///
    /// // Start authorization
    /// let (url, flow_state) = flow.start_authorization_async().await?;
    ///
    /// // ... user completes authorization ...
    ///
    /// // Exchange code (with state validation)
    /// let code = "auth_code_from_callback";
    /// let state = "state_from_callback";
    /// let token = flow.exchange_code(code, Some(state)).await?;
    /// # Ok(())
    /// # }
    /// ```
    #[instrument(skip(self, code, state))]
    pub async fn exchange_code(&self, code: &str, state: Option<&str>) -> Result<TokenInfo> {
        // Get and clear pending state
        let pending_state = {
            let mut pending = self.pending_state.write().await;
            pending.take()
        };

        // Validate state if provided
        if let Some(expected_state) = state {
            match &pending_state {
                Some(flow_state) if flow_state.state != expected_state => {
                    warn!(
                        expected = %flow_state.state,
                        received = %expected_state,
                        "OAuth state mismatch"
                    );
                    return Err(Error::Auth(AuthError::StateMismatch));
                }
                None => {
                    warn!("OAuth state provided but no pending flow state found");
                    return Err(Error::Auth(AuthError::StateMismatch));
                }
                _ => {
                    debug!("OAuth state validated successfully");
                }
            }
        }

        // Get the verifier from pending state
        let verifier = match &pending_state {
            Some(flow_state) => flow_state.code_verifier.clone(),
            None => {
                // If no pending state, try to use code directly
                // This allows callers to provide verifier externally
                warn!("No pending flow state, using empty verifier");
                String::new()
            }
        };

        // Exchange the code for tokens
        let token = oauth::exchange_code(&self.config, code, &verifier).await?;

        // Save the token
        self.storage.save(&token).await?;

        info!("OAuth flow completed successfully");

        Ok(token)
    }

    /// Exchange code using an externally-provided verifier.
    ///
    /// Use this when you've stored the verifier externally rather than
    /// relying on the pending flow state.
    ///
    /// # Arguments
    ///
    /// * `code` - Authorization code from callback
    /// * `verifier` - PKCE code verifier
    /// * `expected_state` - Optional state to validate against
    /// * `received_state` - State received in callback
    #[instrument(skip(self, code, verifier))]
    pub async fn exchange_code_with_verifier(
        &self,
        code: &str,
        verifier: &str,
        expected_state: Option<&str>,
        received_state: Option<&str>,
    ) -> Result<TokenInfo> {
        // Validate state if both are provided
        if let (Some(expected), Some(received)) = (expected_state, received_state) {
            if expected != received {
                warn!(
                    expected = %expected,
                    received = %received,
                    "OAuth state mismatch"
                );
                return Err(Error::Auth(AuthError::StateMismatch));
            }
            debug!("OAuth state validated successfully");
        }

        // Exchange the code for tokens
        let token = oauth::exchange_code(&self.config, code, verifier).await?;

        // Save the token
        self.storage.save(&token).await?;

        info!("OAuth flow completed successfully");

        Ok(token)
    }

    /// Get a valid access token, refreshing if necessary.
    ///
    /// If the stored access token is expired or about to expire,
    /// automatically refreshes it using the refresh token.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Not authenticated (`AuthError::NotAuthenticated`)
    /// - Token refresh fails (`AuthError::InvalidGrant` if revoked)
    /// - Storage access fails
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use gaud::gemini::OAuthFlow;
    /// use gaud::gemini::storage::MemoryTokenStorage;
    ///
    /// # async fn example() -> gaud::gemini::Result<()> {
    /// let storage = MemoryTokenStorage::new();
    /// let flow = OAuthFlow::new(storage);
    ///
    /// // Get token (auto-refreshes if needed)
    /// let access_token = flow.get_access_token().await?;
    /// # Ok(())
    /// # }
    /// ```
    #[instrument(skip(self))]
    pub async fn get_access_token(&self) -> Result<String> {
        let token = self
            .storage
            .load()
            .await?
            .ok_or(Error::Auth(AuthError::NotAuthenticated))?;

        // Check if token is expired
        if token.is_expired() {
            debug!("Access token expired, refreshing");
            let new_token = oauth::refresh_token(&self.config, &token.refresh_token).await?;
            self.storage.save(&new_token).await?;
            return Ok(new_token.access_token);
        }

        Ok(token.access_token)
    }

    /// Get the full TokenInfo, refreshing if necessary.
    ///
    /// Like `get_access_token()` but returns the complete token info
    /// including refresh token and expiry.
    #[instrument(skip(self))]
    pub async fn get_token(&self) -> Result<TokenInfo> {
        let token = self
            .storage
            .load()
            .await?
            .ok_or(Error::Auth(AuthError::NotAuthenticated))?;

        // Check if token is expired
        if token.is_expired() {
            debug!("Access token expired, refreshing");
            let new_token = oauth::refresh_token(&self.config, &token.refresh_token).await?;
            self.storage.save(&new_token).await?;
            return Ok(new_token);
        }

        Ok(token)
    }

    /// Check if the user is currently authenticated.
    ///
    /// Returns `true` if a token exists in storage. Note that this
    /// doesn't verify the token is still valid with Google - use
    /// `get_access_token()` for that.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use gaud::gemini::OAuthFlow;
    /// use gaud::gemini::storage::MemoryTokenStorage;
    ///
    /// # async fn example() -> gaud::gemini::Result<()> {
    /// let storage = MemoryTokenStorage::new();
    /// let flow = OAuthFlow::new(storage);
    ///
    /// if flow.is_authenticated().await? {
    ///     println!("Already authenticated");
    /// } else {
    ///     println!("Need to authenticate");
    /// }
    /// # Ok(())
    /// # }
    /// ```
    #[instrument(skip(self))]
    pub async fn is_authenticated(&self) -> Result<bool> {
        self.storage.exists().await
    }

    /// Log out by removing stored tokens.
    ///
    /// Clears the stored token and any pending flow state.
    /// Does not revoke the token with Google.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use gaud::gemini::OAuthFlow;
    /// use gaud::gemini::storage::MemoryTokenStorage;
    ///
    /// # async fn example() -> gaud::gemini::Result<()> {
    /// let storage = MemoryTokenStorage::new();
    /// let flow = OAuthFlow::new(storage);
    ///
    /// flow.logout().await?;
    /// assert!(!flow.is_authenticated().await?);
    /// # Ok(())
    /// # }
    /// ```
    #[instrument(skip(self))]
    pub async fn logout(&self) -> Result<()> {
        // Clear pending state
        {
            let mut pending = self.pending_state.write().await;
            *pending = None;
        }

        // Remove stored token
        self.storage.remove().await?;

        info!("Logged out successfully");

        Ok(())
    }

    /// Get a reference to the storage backend.
    pub fn storage(&self) -> &S {
        &self.storage
    }

    /// Get the OAuth configuration.
    pub fn config(&self) -> &OAuthConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gemini::storage::MemoryTokenStorage;

    #[tokio::test]
    async fn test_new_flow_not_authenticated() {
        let storage = MemoryTokenStorage::new();
        let flow = OAuthFlow::new(storage);

        assert!(!flow.is_authenticated().await.unwrap());
    }

    #[tokio::test]
    async fn test_start_authorization_returns_url_and_state() {
        let storage = MemoryTokenStorage::new();
        let flow = OAuthFlow::new(storage);

        let (url, state) = flow.start_authorization_async().await.unwrap();

        assert!(url.starts_with("https://accounts.google.com/"));
        assert!(!state.state.is_empty());
        assert!(!state.code_verifier.is_empty());
        assert!(!state.code_challenge.is_empty());
    }

    #[tokio::test]
    async fn test_start_authorization_stores_pending_state() {
        let storage = MemoryTokenStorage::new();
        let flow = OAuthFlow::new(storage);

        let (_, state) = flow.start_authorization_async().await.unwrap();

        // Verify pending state is stored
        let pending = flow.pending_state.read().await;
        assert!(pending.is_some());
        assert_eq!(pending.as_ref().unwrap().state, state.state);
    }

    #[tokio::test]
    async fn test_exchange_code_validates_state_mismatch() {
        let storage = MemoryTokenStorage::new();
        let flow = OAuthFlow::new(storage);

        // Start flow to set pending state
        let (_, _state) = flow.start_authorization_async().await.unwrap();

        // Try to exchange with wrong state
        let result = flow.exchange_code("code", Some("wrong_state")).await;

        assert!(result.is_err());
        match result.unwrap_err() {
            Error::Auth(AuthError::StateMismatch) => {}
            e => panic!("Expected StateMismatch, got: {:?}", e),
        }
    }

    #[tokio::test]
    async fn test_exchange_code_validates_state_no_pending() {
        let storage = MemoryTokenStorage::new();
        let flow = OAuthFlow::new(storage);

        // Don't start flow, so no pending state

        // Try to exchange with state (but no pending state)
        let result = flow.exchange_code("code", Some("some_state")).await;

        assert!(result.is_err());
        match result.unwrap_err() {
            Error::Auth(AuthError::StateMismatch) => {}
            e => panic!("Expected StateMismatch, got: {:?}", e),
        }
    }

    #[tokio::test]
    async fn test_get_access_token_not_authenticated() {
        let storage = MemoryTokenStorage::new();
        let flow = OAuthFlow::new(storage);

        let result = flow.get_access_token().await;

        assert!(result.is_err());
        match result.unwrap_err() {
            Error::Auth(AuthError::NotAuthenticated) => {}
            e => panic!("Expected NotAuthenticated, got: {:?}", e),
        }
    }

    #[tokio::test]
    async fn test_get_access_token_returns_stored_token() {
        let storage = MemoryTokenStorage::new();
        let token = TokenInfo::new("my_access_token".into(), "refresh".into(), 3600);
        storage.save(&token).await.unwrap();

        let flow = OAuthFlow::new(storage);
        let access_token = flow.get_access_token().await.unwrap();

        assert_eq!(access_token, "my_access_token");
    }

    #[tokio::test]
    async fn test_is_authenticated_with_token() {
        let storage = MemoryTokenStorage::new();
        let token = TokenInfo::new("access".into(), "refresh".into(), 3600);
        storage.save(&token).await.unwrap();

        let flow = OAuthFlow::new(storage);

        assert!(flow.is_authenticated().await.unwrap());
    }

    #[tokio::test]
    async fn test_logout_removes_token() {
        let storage = MemoryTokenStorage::new();
        let token = TokenInfo::new("access".into(), "refresh".into(), 3600);
        storage.save(&token).await.unwrap();

        let flow = OAuthFlow::new(storage);

        // Verify authenticated
        assert!(flow.is_authenticated().await.unwrap());

        // Logout
        flow.logout().await.unwrap();

        // Verify not authenticated
        assert!(!flow.is_authenticated().await.unwrap());
    }

    #[tokio::test]
    async fn test_logout_clears_pending_state() {
        let storage = MemoryTokenStorage::new();
        let flow = OAuthFlow::new(storage);

        // Start authorization to set pending state
        let _ = flow.start_authorization_async().await.unwrap();

        // Verify pending state exists
        {
            let pending = flow.pending_state.read().await;
            assert!(pending.is_some());
        }

        // Logout
        flow.logout().await.unwrap();

        // Verify pending state cleared
        {
            let pending = flow.pending_state.read().await;
            assert!(pending.is_none());
        }
    }

    #[tokio::test]
    async fn test_storage_accessor() {
        let storage = MemoryTokenStorage::new();
        let flow = OAuthFlow::new(storage);

        assert_eq!(flow.storage().name(), "memory");
    }

    #[tokio::test]
    async fn test_with_custom_config() {
        let custom_config = OAuthConfig {
            client_id: "custom-client",
            client_secret: "custom-secret",
            auth_url: "https://custom.example.com/auth",
            token_url: "https://custom.example.com/token",
            user_info_url: "https://custom.example.com/userinfo",
            callback_port: 9999,
            scopes: &["custom-scope"],
        };

        let storage = MemoryTokenStorage::new();
        let flow = OAuthFlow::with_config(storage, custom_config);

        let (url, _) = flow.start_authorization_async().await.unwrap();
        assert!(url.starts_with("https://custom.example.com/auth"));
        assert!(url.contains("client_id=custom-client"));
    }
}
