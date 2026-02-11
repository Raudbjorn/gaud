//! OAuth 2.0 with PKCE flow implementation.
//!
//! This module provides the core OAuth functionality:
//! - PKCE code verifier and challenge generation
//! - Authorization URL building
//! - Token exchange and refresh
//!
//! # PKCE Flow
//!
//! 1. Generate code verifier (43-128 URL-safe characters)
//! 2. Derive code challenge using SHA-256 + base64url encoding
//! 3. Build authorization URL with challenge
//! 4. User completes authorization and receives code
//! 5. Exchange code with verifier for tokens
//!
//! # Example
//!
//! ```rust,ignore
//! use antigravity_gate::auth::oauth::{generate_pkce, generate_state, build_authorization_url};
//! use antigravity_gate::DEFAULT_OAUTH_CONFIG;
//!
//! // Generate PKCE pair and state
//! let (verifier, challenge) = generate_pkce();
//! let state = generate_state();
//!
//! // Build the authorization URL
//! let url = build_authorization_url(&DEFAULT_OAUTH_CONFIG, &challenge, &state);
//! println!("Open: {}", url);
//!
//! // After user authorization, exchange the code
//! // let token = exchange_code(&DEFAULT_OAUTH_CONFIG, &code, &verifier).await?;
//! ```

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::Rng;
use sha2::{Digest, Sha256};
use tracing::{debug, instrument, warn};

use crate::auth::TokenInfo;
use crate::constants::OAuthConfig;
use crate::error::{AuthError, Error, Result};

/// State for an in-progress OAuth flow.
///
/// Contains the PKCE verifier, challenge, and state parameter needed
/// to complete the OAuth exchange. This should be stored temporarily
/// while the user completes authorization.
#[derive(Debug, Clone)]
pub struct OAuthFlowState {
    /// The PKCE code verifier (secret, used during token exchange).
    pub code_verifier: String,
    /// The PKCE code challenge (sent in authorization URL).
    pub code_challenge: String,
    /// Random state parameter for CSRF protection.
    pub state: String,
}

impl OAuthFlowState {
    /// Create a new OAuthFlowState with generated PKCE and state values.
    pub fn new() -> Self {
        let (code_verifier, code_challenge) = generate_pkce();
        let state = generate_state();
        Self {
            code_verifier,
            code_challenge,
            state,
        }
    }
}

impl Default for OAuthFlowState {
    fn default() -> Self {
        Self::new()
    }
}

/// Generate a PKCE code verifier and challenge pair.
///
/// The verifier is a cryptographically random 32-byte value encoded as
/// base64url (no padding), resulting in 43 characters. The challenge is
/// the SHA-256 hash of the verifier, also base64url encoded.
///
/// # Returns
///
/// A tuple of `(verifier, challenge)` where:
/// - `verifier`: Secret value to store and send during token exchange
/// - `challenge`: Public value to include in authorization URL
///
/// # Example
///
/// ```
/// use antigravity_gate::auth::oauth::generate_pkce;
///
/// let (verifier, challenge) = generate_pkce();
///
/// // Verifier is 43 URL-safe characters (32 bytes base64url encoded)
/// assert_eq!(verifier.len(), 43);
/// assert!(verifier.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
///
/// // Challenge is also base64url encoded (varies slightly in length)
/// assert!(challenge.len() >= 43);
/// ```
pub fn generate_pkce() -> (String, String) {
    // Generate 32 random bytes for the verifier
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill(&mut bytes);

    // Base64url encode (no padding) to get 43 characters
    let verifier = URL_SAFE_NO_PAD.encode(bytes);

    // SHA-256 hash the verifier and base64url encode
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let hash = hasher.finalize();
    let challenge = URL_SAFE_NO_PAD.encode(hash);

    (verifier, challenge)
}

/// Generate a random state parameter for CSRF protection.
///
/// The state is a 16-byte random value encoded as base64url,
/// resulting in 22 characters. This should be stored and validated
/// when the OAuth callback is received.
///
/// # Example
///
/// ```
/// use antigravity_gate::auth::oauth::generate_state;
///
/// let state1 = generate_state();
/// let state2 = generate_state();
///
/// // States are unique
/// assert_ne!(state1, state2);
///
/// // 16 bytes = 22 base64url characters (no padding)
/// assert_eq!(state1.len(), 22);
/// ```
pub fn generate_state() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Build the Google OAuth authorization URL.
///
/// Constructs the full authorization URL with all required parameters
/// including PKCE challenge, state, and offline access mode.
///
/// # Arguments
///
/// * `config` - OAuth configuration (client ID, URLs, scopes)
/// * `challenge` - PKCE code challenge from `generate_pkce()`
/// * `state` - State parameter from `generate_state()`
///
/// # Returns
///
/// The complete authorization URL for the user to visit.
///
/// # Example
///
/// ```
/// use antigravity_gate::auth::oauth::{generate_pkce, generate_state, build_authorization_url};
/// use antigravity_gate::DEFAULT_OAUTH_CONFIG;
///
/// let (_, challenge) = generate_pkce();
/// let state = generate_state();
/// let url = build_authorization_url(&DEFAULT_OAUTH_CONFIG, &challenge, &state);
///
/// assert!(url.starts_with("https://accounts.google.com/"));
/// assert!(url.contains("client_id="));
/// assert!(url.contains("code_challenge="));
/// assert!(url.contains("code_challenge_method=S256"));
/// assert!(url.contains("access_type=offline"));
/// assert!(url.contains("prompt=consent"));
/// ```
pub fn build_authorization_url(config: &OAuthConfig, challenge: &str, state: &str) -> String {
    let redirect_uri = format!("http://127.0.0.1:{}/callback", config.callback_port);
    let scopes = config.scopes.join(" ");

    format!(
        "{}?client_id={}&redirect_uri={}&response_type=code&scope={}&code_challenge={}&code_challenge_method=S256&state={}&access_type=offline&prompt=consent",
        config.auth_url,
        urlencoding::encode(config.client_id),
        urlencoding::encode(&redirect_uri),
        urlencoding::encode(&scopes),
        urlencoding::encode(challenge),
        urlencoding::encode(state),
    )
}

/// Response from the Google token endpoint.
#[derive(Debug, serde::Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    expires_in: i64,
    // Note: token_type is included in Google's response but not used by us
    #[serde(default)]
    #[allow(dead_code)]
    token_type: Option<String>,
}

/// Error response from the Google token endpoint.
#[derive(Debug, serde::Deserialize)]
struct TokenErrorResponse {
    error: String,
    #[serde(default)]
    error_description: Option<String>,
}

/// Exchange an authorization code for tokens.
///
/// Completes the OAuth flow by exchanging the authorization code
/// (received via callback) for access and refresh tokens.
///
/// # Arguments
///
/// * `config` - OAuth configuration
/// * `code` - Authorization code from the OAuth callback
/// * `verifier` - PKCE code verifier from `generate_pkce()`
///
/// # Errors
///
/// Returns an error if:
/// - The token endpoint returns an error (e.g., invalid code)
/// - Network error occurs
/// - Response cannot be parsed
///
/// # Example
///
/// ```rust,ignore
/// use antigravity_gate::auth::oauth::exchange_code;
/// use antigravity_gate::DEFAULT_OAUTH_CONFIG;
///
/// // After receiving authorization code from callback
/// let token = exchange_code(&DEFAULT_OAUTH_CONFIG, &code, &verifier).await?;
/// println!("Access token obtained, expires in {} seconds", token.time_until_expiry().as_secs());
/// ```
#[instrument(skip(config, code, verifier), fields(token_url = config.token_url))]
pub async fn exchange_code(config: &OAuthConfig, code: &str, verifier: &str) -> Result<TokenInfo> {
    let redirect_uri = format!("http://127.0.0.1:{}/callback", config.callback_port);

    debug!("Exchanging authorization code for tokens");

    let client = reqwest::Client::new();
    let response = client
        .post(config.token_url)
        .form(&[
            ("client_id", config.client_id),
            ("client_secret", config.client_secret),
            ("code", code),
            ("code_verifier", verifier),
            ("grant_type", "authorization_code"),
            ("redirect_uri", &redirect_uri),
        ])
        .send()
        .await?;

    let status = response.status();
    let body = response.text().await?;

    if !status.is_success() {
        // Try to parse error response
        if let Ok(error) = serde_json::from_str::<TokenErrorResponse>(&body) {
            warn!(
                error = %error.error,
                description = ?error.error_description,
                "Token exchange failed"
            );

            if error.error == "invalid_grant" {
                return Err(Error::Auth(AuthError::InvalidGrant));
            }

            return Err(Error::api(
                status.as_u16(),
                error
                    .error_description
                    .unwrap_or_else(|| error.error.clone()),
                None,
            ));
        }

        return Err(Error::api(status.as_u16(), body, None));
    }

    let token_response: TokenResponse = serde_json::from_str(&body)?;

    // refresh_token is required for initial exchange
    let refresh_token = token_response.refresh_token.ok_or_else(|| {
        Error::Auth(AuthError::ProjectDiscovery(
            "No refresh token in response".to_string(),
        ))
    })?;

    debug!("Token exchange successful");

    Ok(TokenInfo::new(
        token_response.access_token,
        refresh_token,
        token_response.expires_in,
    ))
}

/// Refresh an access token using a refresh token.
///
/// Exchanges the refresh token for a new access token. Note that
/// Google may not return a new refresh token on refresh requests.
///
/// # Arguments
///
/// * `config` - OAuth configuration
/// * `refresh_token` - The refresh token (may be composite format)
///
/// # Composite Token Handling
///
/// If the refresh token is in composite format (`refresh|project|managed`),
/// only the base refresh token is sent to Google. Project IDs are preserved
/// and re-attached to the resulting TokenInfo.
///
/// # Errors
///
/// Returns an error if:
/// - The refresh token is invalid or revoked (`AuthError::InvalidGrant`)
/// - Network error occurs
/// - Response cannot be parsed
///
/// # Example
///
/// ```rust,ignore
/// use antigravity_gate::auth::oauth::refresh_token;
/// use antigravity_gate::DEFAULT_OAUTH_CONFIG;
///
/// // Refresh the access token
/// let new_token = refresh_token(&DEFAULT_OAUTH_CONFIG, &old_token.refresh_token).await?;
/// ```
#[instrument(skip(config, refresh_token), fields(token_url = config.token_url))]
pub async fn refresh_token(config: &OAuthConfig, refresh_token: &str) -> Result<TokenInfo> {
    // Parse composite token format if present
    let (base_refresh, project_id, managed_project_id) = parse_composite_token(refresh_token);

    debug!("Refreshing access token");

    let client = reqwest::Client::new();
    let response = client
        .post(config.token_url)
        .form(&[
            ("client_id", config.client_id),
            ("client_secret", config.client_secret),
            ("refresh_token", base_refresh.as_str()),
            ("grant_type", "refresh_token"),
        ])
        .send()
        .await?;

    let status = response.status();
    let body = response.text().await?;

    if !status.is_success() {
        // Try to parse error response
        if let Ok(error) = serde_json::from_str::<TokenErrorResponse>(&body) {
            warn!(
                error = %error.error,
                description = ?error.error_description,
                "Token refresh failed"
            );

            if error.error == "invalid_grant" {
                return Err(Error::Auth(AuthError::InvalidGrant));
            }

            return Err(Error::api(
                status.as_u16(),
                error
                    .error_description
                    .unwrap_or_else(|| error.error.clone()),
                None,
            ));
        }

        return Err(Error::api(status.as_u16(), body, None));
    }

    let token_response: TokenResponse = serde_json::from_str(&body)?;

    debug!("Token refresh successful");

    // Use new refresh token if provided, otherwise preserve the old one
    let new_refresh = token_response
        .refresh_token
        .unwrap_or_else(|| base_refresh.clone());

    let mut token = TokenInfo::new(
        token_response.access_token,
        new_refresh,
        token_response.expires_in,
    );

    // Preserve project IDs from composite token
    if let Some(project) = project_id {
        token = token.with_project_ids(&project, managed_project_id.as_deref());
    }

    Ok(token)
}

/// Parse a composite refresh token into its parts.
///
/// Format: `base_refresh|project_id|managed_project_id`
///
/// Returns (base_refresh, project_id, managed_project_id)
fn parse_composite_token(token: &str) -> (String, Option<String>, Option<String>) {
    let parts: Vec<&str> = token.split('|').collect();
    let base = parts[0].to_string();
    let project = parts
        .get(1)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    let managed = parts
        .get(2)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    (base, project, managed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_pkce_verifier_length() {
        let (verifier, _) = generate_pkce();
        // 32 bytes base64url encoded = 43 characters
        assert_eq!(verifier.len(), 43);
    }

    #[test]
    fn test_generate_pkce_verifier_url_safe() {
        let (verifier, _) = generate_pkce();
        // Should only contain URL-safe characters (no + or /)
        assert!(
            verifier
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "Verifier contains non-URL-safe characters: {}",
            verifier
        );
    }

    #[test]
    fn test_generate_pkce_challenge_deterministic_for_verifier() {
        // Generate a verifier
        let (verifier, challenge1) = generate_pkce();

        // Manually compute challenge from the verifier
        let mut hasher = Sha256::new();
        hasher.update(verifier.as_bytes());
        let hash = hasher.finalize();
        let challenge2 = URL_SAFE_NO_PAD.encode(hash);

        assert_eq!(challenge1, challenge2);
    }

    #[test]
    fn test_generate_pkce_challenge_url_safe() {
        let (_, challenge) = generate_pkce();
        assert!(
            challenge
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "Challenge contains non-URL-safe characters: {}",
            challenge
        );
    }

    #[test]
    fn test_generate_pkce_unique() {
        let (verifier1, challenge1) = generate_pkce();
        let (verifier2, challenge2) = generate_pkce();

        assert_ne!(verifier1, verifier2);
        assert_ne!(challenge1, challenge2);
    }

    #[test]
    fn test_generate_state_length() {
        let state = generate_state();
        // 16 bytes base64url encoded = 22 characters
        assert_eq!(state.len(), 22);
    }

    #[test]
    fn test_generate_state_url_safe() {
        let state = generate_state();
        assert!(
            state
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "State contains non-URL-safe characters: {}",
            state
        );
    }

    #[test]
    fn test_generate_state_unique() {
        let state1 = generate_state();
        let state2 = generate_state();
        assert_ne!(state1, state2);
    }

    #[test]
    fn test_build_authorization_url_contains_required_params() {
        use crate::DEFAULT_OAUTH_CONFIG;

        let (_, challenge) = generate_pkce();
        let state = generate_state();
        let url = build_authorization_url(&DEFAULT_OAUTH_CONFIG, &challenge, &state);

        // Check required parameters are present
        assert!(
            url.starts_with("https://accounts.google.com/"),
            "URL should start with Google auth endpoint"
        );
        assert!(url.contains("client_id="), "URL should contain client_id");
        assert!(
            url.contains("redirect_uri="),
            "URL should contain redirect_uri"
        );
        assert!(
            url.contains("response_type=code"),
            "URL should contain response_type=code"
        );
        assert!(url.contains("scope="), "URL should contain scope");
        assert!(
            url.contains("code_challenge="),
            "URL should contain code_challenge"
        );
        assert!(
            url.contains("code_challenge_method=S256"),
            "URL should contain code_challenge_method=S256"
        );
        assert!(url.contains("state="), "URL should contain state");
        assert!(
            url.contains("access_type=offline"),
            "URL should contain access_type=offline"
        );
        assert!(
            url.contains("prompt=consent"),
            "URL should contain prompt=consent"
        );
    }

    #[test]
    fn test_build_authorization_url_includes_challenge_and_state() {
        use crate::DEFAULT_OAUTH_CONFIG;

        let (_, challenge) = generate_pkce();
        let state = generate_state();
        let url = build_authorization_url(&DEFAULT_OAUTH_CONFIG, &challenge, &state);

        // The challenge and state should be in the URL
        assert!(url.contains(&challenge), "URL should contain the challenge");
        assert!(url.contains(&state), "URL should contain the state");
    }

    #[test]
    fn test_parse_composite_token_simple() {
        let (base, project, managed) = parse_composite_token("refresh_token_here");
        assert_eq!(base, "refresh_token_here");
        assert!(project.is_none());
        assert!(managed.is_none());
    }

    #[test]
    fn test_parse_composite_token_with_project() {
        let (base, project, managed) = parse_composite_token("refresh|proj-123");
        assert_eq!(base, "refresh");
        assert_eq!(project.as_deref(), Some("proj-123"));
        assert!(managed.is_none());
    }

    #[test]
    fn test_parse_composite_token_with_both() {
        let (base, project, managed) = parse_composite_token("refresh|proj-123|managed-456");
        assert_eq!(base, "refresh");
        assert_eq!(project.as_deref(), Some("proj-123"));
        assert_eq!(managed.as_deref(), Some("managed-456"));
    }

    #[test]
    fn test_parse_composite_token_with_empty_parts() {
        // Empty project ID
        let (base, project, managed) = parse_composite_token("refresh||managed-456");
        assert_eq!(base, "refresh");
        assert!(project.is_none());
        assert_eq!(managed.as_deref(), Some("managed-456"));

        // Empty managed project ID
        let (base, project, managed) = parse_composite_token("refresh|proj-123|");
        assert_eq!(base, "refresh");
        assert_eq!(project.as_deref(), Some("proj-123"));
        assert!(managed.is_none());
    }

    #[test]
    fn test_oauth_flow_state_new() {
        let state = OAuthFlowState::new();

        // Verify verifier is valid
        assert_eq!(state.code_verifier.len(), 43);
        assert!(state
            .code_verifier
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));

        // Verify challenge is valid
        assert!(!state.code_challenge.is_empty());

        // Verify state is valid
        assert_eq!(state.state.len(), 22);
    }

    #[test]
    fn test_oauth_flow_state_unique() {
        let state1 = OAuthFlowState::new();
        let state2 = OAuthFlowState::new();

        assert_ne!(state1.code_verifier, state2.code_verifier);
        assert_ne!(state1.code_challenge, state2.code_challenge);
        assert_ne!(state1.state, state2.state);
    }
}
