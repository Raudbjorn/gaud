//! Claude (Anthropic) OAuth PKCE flow.
//!
//! Implements OAuth 2.0 with PKCE for Anthropic's API.
//!
//! # Key Characteristics
//! - Token request format: JSON-encoded (not form-encoded)
//! - Client secret: Not required (PKCE-only)
//! - Auth URL parameter: Requires `code=true`
//! - Redirect: `http://localhost:{port}/oauth/callback/claude`
//!
//! # Endpoints
//! - Authorization: `https://console.anthropic.com/oauth/authorize`
//! - Token: `https://console.anthropic.com/v1/oauth/token`

use serde::Deserialize;
use tracing::{debug, warn};

use super::OAuthError;
use super::pkce::Pkce;
use super::token::TokenInfo;

/// Provider identifier for Claude OAuth.
pub const PROVIDER_ID: &str = "claude";

/// Default authorization URL.
const DEFAULT_AUTH_URL: &str = "https://console.anthropic.com/oauth/authorize";

/// Default token URL.
const DEFAULT_TOKEN_URL: &str = "https://console.anthropic.com/v1/oauth/token";

/// Default scopes for Claude OAuth.
const DEFAULT_SCOPES: &[&str] = &[
    "org:create_api_key",
    "user:profile",
    "user:inference",
];

/// Configuration for the Claude OAuth flow.
#[derive(Debug, Clone)]
pub struct ClaudeOAuthConfig {
    pub client_id: String,
    pub auth_url: String,
    pub token_url: String,
    pub redirect_uri: String,
    pub scopes: Vec<String>,
}

impl ClaudeOAuthConfig {
    /// Create a Claude OAuth config from the application config.
    pub fn from_provider_config(
        client_id: &str,
        auth_url: &str,
        callback_port: u16,
    ) -> Self {
        Self {
            client_id: client_id.to_string(),
            auth_url: auth_url.to_string(),
            token_url: DEFAULT_TOKEN_URL.to_string(),
            redirect_uri: format!("http://localhost:{}/oauth/callback/claude", callback_port),
            scopes: DEFAULT_SCOPES.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// Create a default Claude OAuth config (requires client_id and callback_port).
    pub fn new(client_id: &str, callback_port: u16) -> Self {
        Self::from_provider_config(client_id, DEFAULT_AUTH_URL, callback_port)
    }
}

/// Build the Claude authorization URL.
///
/// Claude requires a special `code=true` parameter in addition to the
/// standard OAuth parameters.
pub fn build_authorize_url(config: &ClaudeOAuthConfig, pkce: &Pkce, state: &str) -> String {
    let scopes = config.scopes.join(" ");
    format!(
        "{}?code=true&response_type=code&client_id={}&redirect_uri={}&scope={}&code_challenge={}&code_challenge_method=S256&state={}",
        config.auth_url,
        urlencoding::encode(&config.client_id),
        urlencoding::encode(&config.redirect_uri),
        urlencoding::encode(&scopes),
        urlencoding::encode(&pkce.challenge),
        urlencoding::encode(state),
    )
}

/// Token response from Claude's token endpoint.
#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    expires_in: i64,
}

/// Error response from Claude's token endpoint.
#[derive(Debug, Deserialize)]
struct TokenErrorResponse {
    error: String,
    #[serde(default)]
    error_description: Option<String>,
}

/// Exchange an authorization code for tokens.
///
/// Claude's token endpoint expects JSON-encoded requests, not form-encoded.
pub async fn exchange_code(
    http_client: &reqwest::Client,
    config: &ClaudeOAuthConfig,
    code: &str,
    verifier: &str,
) -> Result<TokenInfo, OAuthError> {
    debug!("Exchanging authorization code for Claude tokens");

    let request_body = serde_json::json!({
        "grant_type": "authorization_code",
        "client_id": config.client_id,
        "code": code,
        "code_verifier": verifier,
        "redirect_uri": config.redirect_uri,
    });

    let response = http_client
        .post(&config.token_url)
        .header("Content-Type", "application/json")
        .json(&request_body)
        .send()
        .await?;

    let status = response.status();
    let body = response.text().await?;

    if !status.is_success() {
        if let Ok(error) = serde_json::from_str::<TokenErrorResponse>(&body) {
            warn!(
                error = %error.error,
                description = ?error.error_description,
                "Claude token exchange failed"
            );
            return Err(OAuthError::ExchangeFailed(
                error.error_description.unwrap_or(error.error),
            ));
        }
        return Err(OAuthError::ExchangeFailed(format!(
            "HTTP {}: {}",
            status.as_u16(),
            body
        )));
    }

    let token_response: TokenResponse = serde_json::from_str(&body).map_err(|e| {
        OAuthError::ExchangeFailed(format!("Failed to parse token response: {}", e))
    })?;

    let refresh_token = token_response.refresh_token.ok_or_else(|| {
        OAuthError::ExchangeFailed("No refresh token in response".to_string())
    })?;

    debug!("Claude token exchange successful");

    Ok(TokenInfo::new(
        token_response.access_token,
        Some(refresh_token),
        Some(token_response.expires_in),
        PROVIDER_ID,
    ))
}

/// Refresh the Claude access token.
///
/// Uses JSON body for token refresh (same as exchange).
pub async fn refresh_token(
    http_client: &reqwest::Client,
    config: &ClaudeOAuthConfig,
    refresh_token_value: &str,
) -> Result<TokenInfo, OAuthError> {
    // Parse composite token format if present
    let parts: Vec<&str> = refresh_token_value.split('|').collect();
    let base_refresh = parts[0];

    debug!("Refreshing Claude access token");

    let request_body = serde_json::json!({
        "grant_type": "refresh_token",
        "client_id": config.client_id,
        "refresh_token": base_refresh,
    });

    let response = http_client
        .post(&config.token_url)
        .header("Content-Type", "application/json")
        .json(&request_body)
        .send()
        .await?;

    let status = response.status();
    let body = response.text().await?;

    if !status.is_success() {
        if let Ok(error) = serde_json::from_str::<TokenErrorResponse>(&body) {
            warn!(
                error = %error.error,
                description = ?error.error_description,
                "Claude token refresh failed"
            );
            if error.error == "invalid_grant" {
                return Err(OAuthError::TokenExpired(PROVIDER_ID.to_string()));
            }
            return Err(OAuthError::ExchangeFailed(
                error.error_description.unwrap_or(error.error),
            ));
        }
        return Err(OAuthError::ExchangeFailed(format!(
            "HTTP {}: {}",
            status.as_u16(),
            body
        )));
    }

    let token_response: TokenResponse = serde_json::from_str(&body).map_err(|e| {
        OAuthError::ExchangeFailed(format!("Failed to parse refresh response: {}", e))
    })?;

    debug!("Claude token refresh successful");

    // Use new refresh token if provided, otherwise preserve the old one
    let new_refresh = token_response
        .refresh_token
        .unwrap_or_else(|| base_refresh.to_string());

    let mut token = TokenInfo::new(
        token_response.access_token,
        Some(new_refresh),
        Some(token_response.expires_in),
        PROVIDER_ID,
    );

    // Preserve project IDs from composite token
    let project_id = parts.get(1).filter(|s| !s.is_empty()).map(|s| s.to_string());
    let managed_project_id = parts.get(2).filter(|s| !s.is_empty()).map(|s| s.to_string());
    if let Some(project) = project_id {
        token = token.with_project_ids(&project, managed_project_id.as_deref());
    }

    Ok(token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_authorize_url_contains_code_true() {
        let config = ClaudeOAuthConfig::new("test-client-id", 19284);
        let pkce = Pkce::generate();
        let url = build_authorize_url(&config, &pkce, "test_state");

        assert!(url.contains("code=true"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("client_id="));
        assert!(url.contains("redirect_uri="));
        assert!(url.contains("scope="));
        assert!(url.contains("code_challenge="));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("state=test_state"));
    }

    #[test]
    fn test_build_authorize_url_contains_pkce_challenge() {
        let config = ClaudeOAuthConfig::new("test-client-id", 19284);
        let pkce = Pkce::generate();
        let url = build_authorize_url(&config, &pkce, "state");
        assert!(url.contains(&pkce.challenge));
    }

    #[test]
    fn test_build_authorize_url_encodes_special_chars() {
        let config = ClaudeOAuthConfig::new("test-client-id", 19284);
        let pkce = Pkce::generate();
        let url = build_authorize_url(&config, &pkce, "state with spaces");
        assert!(url.contains("state%20with%20spaces") || url.contains("state+with+spaces"));
    }

    #[test]
    fn test_config_from_provider() {
        let config = ClaudeOAuthConfig::from_provider_config(
            "my-client",
            "https://custom.auth.url",
            8080,
        );
        assert_eq!(config.client_id, "my-client");
        assert_eq!(config.auth_url, "https://custom.auth.url");
        assert_eq!(config.redirect_uri, "http://localhost:8080/oauth/callback/claude");
        assert!(!config.scopes.is_empty());
    }

    #[test]
    fn test_config_new_uses_defaults() {
        let config = ClaudeOAuthConfig::new("my-client", 19284);
        assert_eq!(config.auth_url, DEFAULT_AUTH_URL);
        assert_eq!(config.token_url, DEFAULT_TOKEN_URL);
    }
}
