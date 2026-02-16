//! Gemini (Google Cloud) OAuth PKCE flow.
//!
//! Implements OAuth 2.0 with PKCE for Google's Cloud Platform API (Gemini access)
//! using the [`oauth2`] crate for type-safe token exchange and refresh.
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

use oauth2::TokenResponse as _;
use oauth2::basic::BasicClient;
use oauth2::{
    AuthType, AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, PkceCodeChallenge,
    PkceCodeVerifier, RedirectUrl, RefreshToken, Scope, TokenUrl,
};
use tracing::debug;

use super::OAuthClient;
use super::OAuthError;
use super::token::TokenInfo;

/// Provider identifier for Gemini OAuth.
pub const PROVIDER_ID: &str = "gemini";

/// Default authorization URL.
const DEFAULT_AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";

/// Default token URL.
const DEFAULT_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

/// Default scopes for Gemini/Cloud Platform OAuth.
const DEFAULT_SCOPES: &[&str] = &[
    "https://www.googleapis.com/auth/cloud-platform",
    "https://www.googleapis.com/auth/userinfo.email",
    "https://www.googleapis.com/auth/userinfo.profile",
    "https://www.googleapis.com/auth/cclog",
    "https://www.googleapis.com/auth/experimentsandconfigs",
];

/// Configuration for the Gemini OAuth flow.
#[derive(Clone)]
pub struct GeminiOAuthConfig {
    pub client_id: String,
    pub client_secret: String,
    pub auth_url: String,
    pub token_url: String,
    pub redirect_uri: String,
    pub scopes: Vec<String>,
}

impl std::fmt::Debug for GeminiOAuthConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GeminiOAuthConfig")
            .field("client_id", &self.client_id)
            .field("client_secret", &"[REDACTED]")
            .field("auth_url", &self.auth_url)
            .field("token_url", &self.token_url)
            .field("redirect_uri", &self.redirect_uri)
            .field("scopes", &self.scopes)
            .finish()
    }
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
            redirect_uri: format!("http://127.0.0.1:{}/oauth/callback/gemini", callback_port),
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

// =============================================================================
// Helpers
// =============================================================================

/// Build a typed `BasicClient` from a `GeminiOAuthConfig`.
///
/// Google expects client credentials in the request body (not Basic Auth header),
/// so we set `AuthType::RequestBody`.
fn build_oauth2_client(config: &GeminiOAuthConfig) -> Result<OAuthClient, OAuthError> {
    let client_id = ClientId::new(config.client_id.clone());
    let client_secret = ClientSecret::new(config.client_secret.clone());
    let auth_url = AuthUrl::new(config.auth_url.clone())
        .map_err(|e| OAuthError::Other(format!("Invalid auth URL: {}", e)))?;
    let token_url = TokenUrl::new(config.token_url.clone())
        .map_err(|e| OAuthError::Other(format!("Invalid token URL: {}", e)))?;
    let redirect_url = RedirectUrl::new(config.redirect_uri.clone())
        .map_err(|e| OAuthError::Other(format!("Invalid redirect URI: {}", e)))?;

    let client = BasicClient::new(client_id)
        .set_client_secret(client_secret)
        .set_auth_uri(auth_url)
        .set_token_uri(token_url)
        .set_redirect_uri(redirect_url)
        .set_auth_type(AuthType::RequestBody);

    Ok(client)
}

/// Map oauth2 errors to OAuthError, delegating to the shared helper.
fn map_token_error<RE: std::error::Error + 'static>(
    err: oauth2::RequestTokenError<
        RE,
        oauth2::StandardErrorResponse<oauth2::basic::BasicErrorResponseType>,
    >,
) -> OAuthError {
    super::map_oauth_token_error(PROVIDER_ID, err)
}

// =============================================================================
// Public API
// =============================================================================

/// Build the Gemini authorization URL with PKCE.
///
/// Generates a PKCE challenge/verifier pair internally and returns both the
/// authorization URL and the PKCE verifier secret (which must be stored for
/// the token exchange step).
///
/// Google OAuth requires:
/// - `access_type=offline` to receive a refresh token
/// - `prompt=consent` to force consent screen, ensuring refresh token is returned
pub fn build_authorize_url(
    config: &GeminiOAuthConfig,
    state: &str,
) -> Result<(String, String), OAuthError> {
    let client = build_oauth2_client(config)?;
    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();

    let mut auth_request = client
        .authorize_url(|| CsrfToken::new(state.to_string()))
        .set_pkce_challenge(pkce_challenge)
        .add_extra_param("access_type", "offline")
        .add_extra_param("prompt", "consent");

    for scope in &config.scopes {
        auth_request = auth_request.add_scope(Scope::new(scope.clone()));
    }

    let (url, _csrf_token) = auth_request.url();

    Ok((url.to_string(), pkce_verifier.secret().to_string()))
}

/// Exchange an authorization code for tokens.
///
/// Uses the oauth2 crate's typed `exchange_code` with PKCE verifier completion.
pub async fn exchange_code(
    http_client: &reqwest::Client,
    config: &GeminiOAuthConfig,
    code: &str,
    verifier: &str,
) -> Result<TokenInfo, OAuthError> {
    debug!("Exchanging authorization code for Gemini tokens");

    let client = build_oauth2_client(config)?;
    let http_client = http_client.clone(); // cheap Arc clone for owned AsyncHttpClient

    let token_response = client
        .exchange_code(AuthorizationCode::new(code.to_string()))
        .set_pkce_verifier(PkceCodeVerifier::new(verifier.to_string()))
        .request_async(&http_client)
        .await
        .map_err(map_token_error)?;

    let access_token = token_response.access_token().secret().to_string();
    let refresh_token = token_response
        .refresh_token()
        .map(|rt| rt.secret().to_string())
        .ok_or_else(|| {
            OAuthError::ExchangeFailed(
                "No refresh token in response - ensure access_type=offline and prompt=consent"
                    .to_string(),
            )
        })?;
    let expires_in = token_response.expires_in().map(|d| d.as_secs() as i64);

    debug!("Gemini token exchange successful");

    Ok(TokenInfo::new(
        access_token,
        Some(refresh_token),
        expires_in,
        PROVIDER_ID,
    ))
}

/// Refresh the Gemini access token.
///
/// Handles composite token format (`refresh|project_id|managed_project_id`),
/// using only the base refresh token for the request and re-attaching
/// project IDs to the result.
pub async fn refresh_token(
    http_client: &reqwest::Client,
    config: &GeminiOAuthConfig,
    refresh_token_value: &str,
) -> Result<TokenInfo, OAuthError> {
    // Parse composite token format if present
    let parts: Vec<&str> = refresh_token_value.split('|').collect();
    let base_refresh = parts[0];

    debug!("Refreshing Gemini access token");

    let client = build_oauth2_client(config)?;
    let http_client = http_client.clone(); // cheap Arc clone for owned AsyncHttpClient

    let token_response = client
        .exchange_refresh_token(&RefreshToken::new(base_refresh.to_string()))
        .request_async(&http_client)
        .await
        .map_err(map_token_error)?;

    debug!("Gemini token refresh successful");

    let access_token = token_response.access_token().secret().to_string();
    // Google typically doesn't return a new refresh token on refresh
    let new_refresh = token_response
        .refresh_token()
        .map(|rt| rt.secret().to_string())
        .unwrap_or_else(|| base_refresh.to_string());
    let expires_in = token_response.expires_in().map(|d| d.as_secs() as i64);

    let mut token = TokenInfo::new(access_token, Some(new_refresh), expires_in, PROVIDER_ID);

    // Preserve project IDs from composite token
    let project_id = parts
        .get(1)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    let managed_project_id = parts
        .get(2)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    if let Some(project) = project_id {
        token = token.with_project_ids(&project, managed_project_id.as_deref());
    }

    Ok(token)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a `GeminiOAuthConfig` whose `token_url` points at the given wiremock server.
    fn mock_config(mock_server_uri: &str) -> GeminiOAuthConfig {
        GeminiOAuthConfig::from_provider_config(
            "test-client",
            "test-secret",
            DEFAULT_AUTH_URL,
            mock_server_uri,
            19285,
        )
    }

    /// Shared test HTTP client (reused across tests like `OAuthManager` does).
    fn test_http_client() -> reqwest::Client {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .unwrap()
    }

    /// Standard successful token response JSON from Google.
    fn success_token_json() -> serde_json::Value {
        serde_json::json!({
            "access_token": "ya29.test-access-token",
            "token_type": "Bearer",
            "expires_in": 3600,
            "refresh_token": "1//test-refresh-token",
            "scope": "https://www.googleapis.com/auth/cloud-platform"
        })
    }

    /// Successful token response without a refresh token (happens on refresh).
    fn success_no_refresh_json() -> serde_json::Value {
        serde_json::json!({
            "access_token": "ya29.new-access-token",
            "token_type": "Bearer",
            "expires_in": 3600,
            "scope": "https://www.googleapis.com/auth/cloud-platform"
        })
    }

    #[test]
    fn test_build_authorize_url_contains_offline_access() {
        let config = GeminiOAuthConfig::new("test-client", "test-secret", 19285);
        let (url, _verifier) = build_authorize_url(&config, "test_state").unwrap();

        assert!(url.contains("access_type=offline"));
        assert!(url.contains("prompt=consent"));
    }

    #[test]
    fn test_build_authorize_url_contains_standard_params() {
        let config = GeminiOAuthConfig::new("test-client", "test-secret", 19285);
        let (url, _verifier) = build_authorize_url(&config, "test_state").unwrap();

        assert!(url.contains("response_type=code"));
        assert!(url.contains("client_id="));
        assert!(url.contains("redirect_uri="));
        assert!(url.contains("scope="));
        assert!(url.contains("code_challenge="));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("state=test_state"));
    }

    #[test]
    fn test_build_authorize_url_returns_verifier() {
        let config = GeminiOAuthConfig::new("test-client", "test-secret", 19285);
        let (url, verifier) = build_authorize_url(&config, "state").unwrap();

        // Verifier should be non-empty
        assert!(!verifier.is_empty());
        // URL should contain a code_challenge derived from the verifier
        assert!(url.contains("code_challenge="));
    }

    #[test]
    fn test_build_authorize_url_unique_verifiers() {
        let config = GeminiOAuthConfig::new("test-client", "test-secret", 19285);
        let (_url1, verifier1) = build_authorize_url(&config, "state1").unwrap();
        let (_url2, verifier2) = build_authorize_url(&config, "state2").unwrap();

        assert_ne!(verifier1, verifier2);
    }

    #[test]
    fn test_build_authorize_url_starts_with_google() {
        let config = GeminiOAuthConfig::new("test-client", "test-secret", 19285);
        let (url, _verifier) = build_authorize_url(&config, "state").unwrap();
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
        assert_eq!(
            config.redirect_uri,
            "http://127.0.0.1:8080/oauth/callback/gemini"
        );
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
        assert!(config.redirect_uri.contains("127.0.0.1"));
    }

    // =========================================================================
    // Async wiremock tests for exchange_code and refresh_token
    // =========================================================================

    #[tokio::test]
    async fn test_exchange_code_success() {
        let mock_server = wiremock::MockServer::start().await;

        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(success_token_json()))
            .mount(&mock_server)
            .await;

        let config = mock_config(&mock_server.uri());
        let client = test_http_client();
        let result = exchange_code(&client, &config, "test-auth-code", "test-verifier").await;

        let token = result.expect("exchange_code should succeed");
        assert_eq!(token.access_token, "ya29.test-access-token");
        assert_eq!(
            token.refresh_token.as_deref(),
            Some("1//test-refresh-token")
        );
        assert!(token.expires_at.is_some());
        assert_eq!(token.provider, PROVIDER_ID);
        assert!(!token.is_expired());
    }

    #[tokio::test]
    async fn test_exchange_code_missing_refresh_token() {
        let mock_server = wiremock::MockServer::start().await;

        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(success_no_refresh_json()),
            )
            .mount(&mock_server)
            .await;

        let config = mock_config(&mock_server.uri());
        let client = test_http_client();
        let result = exchange_code(&client, &config, "test-auth-code", "test-verifier").await;

        let err = result.expect_err("should fail without refresh token");
        match err {
            OAuthError::ExchangeFailed(msg) => {
                assert!(
                    msg.contains("refresh token"),
                    "Error should mention refresh token: {msg}"
                );
            }
            other => panic!("Expected ExchangeFailed, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_exchange_code_invalid_grant() {
        let mock_server = wiremock::MockServer::start().await;

        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/"))
            .respond_with(
                wiremock::ResponseTemplate::new(400).set_body_json(serde_json::json!({
                    "error": "invalid_grant",
                    "error_description": "Code has expired"
                })),
            )
            .mount(&mock_server)
            .await;

        let config = mock_config(&mock_server.uri());
        let client = test_http_client();
        let result = exchange_code(&client, &config, "expired-code", "test-verifier").await;

        let err = result.expect_err("should fail with invalid_grant");
        assert!(
            matches!(err, OAuthError::TokenExpired(_)),
            "Expected TokenExpired, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn test_exchange_code_server_error() {
        let mock_server = wiremock::MockServer::start().await;

        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/"))
            .respond_with(
                wiremock::ResponseTemplate::new(400).set_body_json(serde_json::json!({
                    "error": "invalid_client",
                    "error_description": "The OAuth client was not found."
                })),
            )
            .mount(&mock_server)
            .await;

        let config = mock_config(&mock_server.uri());
        let client = test_http_client();
        let result = exchange_code(&client, &config, "test-code", "test-verifier").await;

        let err = result.expect_err("should fail with server error");
        match err {
            OAuthError::ExchangeFailed(msg) => {
                assert!(
                    msg.contains("client was not found"),
                    "Error should contain description: {msg}"
                );
            }
            other => panic!("Expected ExchangeFailed, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_refresh_token_success() {
        let mock_server = wiremock::MockServer::start().await;

        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(success_no_refresh_json()),
            )
            .mount(&mock_server)
            .await;

        let config = mock_config(&mock_server.uri());
        let client = test_http_client();
        let result = refresh_token(&client, &config, "1//original-refresh").await;

        let token = result.expect("refresh should succeed");
        assert_eq!(token.access_token, "ya29.new-access-token");
        // Google doesn't return new refresh on refresh; should preserve original
        assert_eq!(token.refresh_token.as_deref(), Some("1//original-refresh"));
        assert!(token.expires_at.is_some());
        assert_eq!(token.provider, PROVIDER_ID);
    }

    #[tokio::test]
    async fn test_refresh_token_returns_new_refresh() {
        let mock_server = wiremock::MockServer::start().await;

        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(success_token_json()))
            .mount(&mock_server)
            .await;

        let config = mock_config(&mock_server.uri());
        let client = test_http_client();
        let result = refresh_token(&client, &config, "old-refresh").await;

        let token = result.expect("refresh should succeed");
        // When server returns a new refresh token, use it
        assert_eq!(
            token.refresh_token.as_deref(),
            Some("1//test-refresh-token")
        );
    }

    #[tokio::test]
    async fn test_refresh_token_invalid_grant() {
        let mock_server = wiremock::MockServer::start().await;

        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/"))
            .respond_with(
                wiremock::ResponseTemplate::new(400).set_body_json(serde_json::json!({
                    "error": "invalid_grant",
                    "error_description": "Token has been expired or revoked."
                })),
            )
            .mount(&mock_server)
            .await;

        let config = mock_config(&mock_server.uri());
        let client = test_http_client();
        let result = refresh_token(&client, &config, "revoked-refresh").await;

        let err = result.expect_err("should fail with invalid_grant");
        assert!(
            matches!(err, OAuthError::TokenExpired(_)),
            "Expected TokenExpired, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn test_refresh_token_preserves_composite_format() {
        let mock_server = wiremock::MockServer::start().await;

        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(success_no_refresh_json()),
            )
            .mount(&mock_server)
            .await;

        let config = mock_config(&mock_server.uri());
        let client = test_http_client();
        // Composite token: refresh|project_id|managed_project_id
        let result = refresh_token(&client, &config, "1//base-refresh|proj-123|managed-456").await;

        let token = result.expect("refresh should succeed");
        assert_eq!(token.access_token, "ya29.new-access-token");

        // Project IDs should be preserved in the composite token
        let (base, project, managed) = token.parse_refresh_parts();
        assert_eq!(base, "1//base-refresh");
        assert_eq!(project.as_deref(), Some("proj-123"));
        assert_eq!(managed.as_deref(), Some("managed-456"));
    }

    #[tokio::test]
    async fn test_refresh_token_preserves_partial_composite() {
        let mock_server = wiremock::MockServer::start().await;

        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(success_no_refresh_json()),
            )
            .mount(&mock_server)
            .await;

        let config = mock_config(&mock_server.uri());
        let client = test_http_client();
        // Composite with only project_id (no managed)
        let result = refresh_token(&client, &config, "1//base-refresh|proj-only").await;

        let token = result.expect("refresh should succeed");
        let (base, project, managed) = token.parse_refresh_parts();
        assert_eq!(base, "1//base-refresh");
        assert_eq!(project.as_deref(), Some("proj-only"));
        assert!(managed.is_none());
    }
}
