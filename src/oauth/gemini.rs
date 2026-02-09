//! Gemini (Google Cloud) OAuth PKCE flow.
//!
//! Implements OAuth 2.0 with PKCE for Google's Cloud Platform API (Gemini access).
//!
//! # Key Characteristics
//! - Token request format: Form-encoded (standard OAuth)
//! - Client secret: Required (even with PKCE)
//! - Auth URL parameters: Requires `access_type=offline` and `prompt=consent`
//! - Redirect: `http://localhost:{port}/oauth/callback/gemini`
//!
//! # Endpoints
//! - Authorization: `https://accounts.google.com/o/oauth2/v2/auth`
//! - Token: `https://oauth2.googleapis.com/token`

use serde::Deserialize;
use tracing::{debug, warn};

use super::OAuthError;
use super::pkce::Pkce;
use super::token::TokenInfo;

/// Provider identifier for Gemini OAuth.
pub const PROVIDER_ID: &str = "gemini";

/// Default authorization URL.
const DEFAULT_AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";

/// Default token URL.
const DEFAULT_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

/// Default scopes for Gemini/Cloud Platform OAuth.
const DEFAULT_SCOPES: &[&str] = &["https://www.googleapis.com/auth/cloud-platform"];

/// Configuration for the Gemini OAuth flow.
#[derive(Debug, Clone)]
pub struct GeminiOAuthConfig {
    pub client_id: String,
    pub client_secret: String,
    pub auth_url: String,
    pub token_url: String,
    pub redirect_uri: String,
    pub scopes: Vec<String>,
}

impl GeminiOAuthConfig {
    /// Create a Gemini OAuth config from the application config.
    pub fn from_provider_config(
        client_id: &str,
        client_secret: &str,
        auth_url: &str,
        token_url: &str,
        callback_port: u16,
    ) -> Self {
        Self {
            client_id: client_id.to_string(),
            client_secret: client_secret.to_string(),
            auth_url: auth_url.to_string(),
            token_url: token_url.to_string(),
            redirect_uri: format!("http://localhost:{}/oauth/callback/gemini", callback_port),
            scopes: DEFAULT_SCOPES.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// Create a Gemini OAuth config with default endpoints.
    pub fn new(client_id: &str, client_secret: &str, callback_port: u16) -> Self {
        Self::from_provider_config(
            client_id,
            client_secret,
            DEFAULT_AUTH_URL,
            DEFAULT_TOKEN_URL,
            callback_port,
        )
    }
}

/// Build the Gemini authorization URL.
///
/// Google OAuth requires:
/// - `access_type=offline` to receive a refresh token
/// - `prompt=consent` to force consent screen, ensuring refresh token is returned
pub fn build_authorize_url(config: &GeminiOAuthConfig, pkce: &Pkce, state: &str) -> String {
    let scopes = config.scopes.join(" ");
    format!(
        "{}?client_id={}&redirect_uri={}&response_type=code&scope={}&code_challenge={}&code_challenge_method=S256&state={}&access_type=offline&prompt=consent",
        config.auth_url,
        urlencoding::encode(&config.client_id),
        urlencoding::encode(&config.redirect_uri),
        urlencoding::encode(&scopes),
        urlencoding::encode(&pkce.challenge),
        urlencoding::encode(state),
    )
}

/// Token response from Google's token endpoint.
#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    expires_in: i64,
}

/// Error response from Google's token endpoint.
#[derive(Debug, Deserialize)]
struct TokenErrorResponse {
    error: String,
    #[serde(default)]
    error_description: Option<String>,
}

/// Exchange an authorization code for tokens.
///
/// Google's token endpoint expects standard form-encoded requests,
/// including the client_secret.
pub async fn exchange_code(
    http_client: &reqwest::Client,
    config: &GeminiOAuthConfig,
    code: &str,
    verifier: &str,
) -> Result<TokenInfo, OAuthError> {
    debug!("Exchanging authorization code for Gemini tokens");

    let form_data = [
        ("code", code),
        ("code_verifier", verifier),
        ("grant_type", "authorization_code"),
        ("redirect_uri", &config.redirect_uri),
        ("client_id", &config.client_id),
        ("client_secret", &config.client_secret),
    ];

    let response = http_client
        .post(&config.token_url)
        .form(&form_data)
        .send()
        .await?;

    let status = response.status();
    let body = response.text().await?;

    if !status.is_success() {
        if let Ok(error) = serde_json::from_str::<TokenErrorResponse>(&body) {
            warn!(
                error = %error.error,
                description = ?error.error_description,
                "Gemini token exchange failed"
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
        OAuthError::ExchangeFailed(
            "No refresh token in response - ensure access_type=offline and prompt=consent"
                .to_string(),
        )
    })?;

    debug!("Gemini token exchange successful");

    Ok(TokenInfo::new(
        token_response.access_token,
        Some(refresh_token),
        Some(token_response.expires_in),
        PROVIDER_ID,
    ))
}

/// Refresh the Gemini access token.
///
/// Uses form-encoded body with client_secret.
pub async fn refresh_token(
    http_client: &reqwest::Client,
    config: &GeminiOAuthConfig,
    refresh_token_value: &str,
) -> Result<TokenInfo, OAuthError> {
    // Parse composite token format if present
    let parts: Vec<&str> = refresh_token_value.split('|').collect();
    let base_refresh = parts[0];

    debug!("Refreshing Gemini access token");

    let form_data = [
        ("refresh_token", base_refresh),
        ("grant_type", "refresh_token"),
        ("client_id", config.client_id.as_str()),
        ("client_secret", config.client_secret.as_str()),
    ];

    let response = http_client
        .post(&config.token_url)
        .form(&form_data)
        .send()
        .await?;

    let status = response.status();
    let body = response.text().await?;

    if !status.is_success() {
        if let Ok(error) = serde_json::from_str::<TokenErrorResponse>(&body) {
            warn!(
                error = %error.error,
                description = ?error.error_description,
                "Gemini token refresh failed"
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

    debug!("Gemini token refresh successful");

    // Google typically doesn't return a new refresh token on refresh
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
    fn test_build_authorize_url_contains_offline_access() {
        let config = GeminiOAuthConfig::new("test-client", "test-secret", 19285);
        let pkce = Pkce::generate();
        let url = build_authorize_url(&config, &pkce, "test_state");

        assert!(url.contains("access_type=offline"));
        assert!(url.contains("prompt=consent"));
    }

    #[test]
    fn test_build_authorize_url_contains_standard_params() {
        let config = GeminiOAuthConfig::new("test-client", "test-secret", 19285);
        let pkce = Pkce::generate();
        let url = build_authorize_url(&config, &pkce, "test_state");

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
        let config = GeminiOAuthConfig::new("test-client", "test-secret", 19285);
        let pkce = Pkce::generate();
        let url = build_authorize_url(&config, &pkce, "state");
        assert!(url.contains(&pkce.challenge));
    }

    #[test]
    fn test_build_authorize_url_starts_with_google() {
        let config = GeminiOAuthConfig::new("test-client", "test-secret", 19285);
        let pkce = Pkce::generate();
        let url = build_authorize_url(&config, &pkce, "state");
        assert!(url.starts_with("https://accounts.google.com/"));
    }

    #[test]
    fn test_config_from_provider() {
        let config = GeminiOAuthConfig::from_provider_config(
            "my-client",
            "my-secret",
            "https://custom.auth.url",
            "https://custom.token.url",
            8080,
        );
        assert_eq!(config.client_id, "my-client");
        assert_eq!(config.client_secret, "my-secret");
        assert_eq!(config.auth_url, "https://custom.auth.url");
        assert_eq!(config.token_url, "https://custom.token.url");
        assert_eq!(config.redirect_uri, "http://localhost:8080/oauth/callback/gemini");
    }

    #[test]
    fn test_config_new_uses_defaults() {
        let config = GeminiOAuthConfig::new("my-client", "my-secret", 19285);
        assert_eq!(config.auth_url, DEFAULT_AUTH_URL);
        assert_eq!(config.token_url, DEFAULT_TOKEN_URL);
    }

    #[test]
    fn test_redirect_uri_uses_callback_port() {
        let config = GeminiOAuthConfig::new("my-client", "my-secret", 9999);
        assert!(config.redirect_uri.contains("9999"));
        assert!(config.redirect_uri.contains("localhost"));
    }
}
