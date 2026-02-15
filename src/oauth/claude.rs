//! Claude (Anthropic) OAuth PKCE flow.
//!
//! Implements OAuth 2.0 with PKCE for Anthropic's API using the [`oauth2`]
//! crate for type-safe token exchange and refresh.
//!
//! # Key Characteristics
//! - Token request format: JSON-encoded (custom HTTP client converts form->JSON)
//! - Client secret: Not required (PKCE-only)
//! - Auth URL parameter: Requires `code=true`
//! - Redirect: `http://localhost:{port}/oauth/callback/claude`
//!
//! # Endpoints
//! - Authorization: `https://claude.ai/oauth/authorize`
//! - Token: `https://console.anthropic.com/v1/oauth/token`

use std::future::Future;
use std::pin::Pin;

use oauth2::TokenResponse as _;
use oauth2::basic::BasicClient;
use oauth2::{
    AuthType, AuthUrl, AuthorizationCode, ClientId, CsrfToken, HttpClientError,
    PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, RefreshToken, Scope, TokenUrl,
};
use tracing::{debug, warn};

use super::OAuthError;
use super::OAuthClient;
use super::token::TokenInfo;

/// Provider identifier for Claude OAuth.
pub const PROVIDER_ID: &str = "claude";

/// Default authorization URL.
const DEFAULT_AUTH_URL: &str = "https://claude.ai/oauth/authorize";

/// Default token URL.
const DEFAULT_TOKEN_URL: &str = "https://console.anthropic.com/v1/oauth/token";

/// Default scopes for Claude OAuth.
const DEFAULT_SCOPES: &[&str] = &["org:create_api_key", "user:profile", "user:inference"];

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
    pub fn from_provider_config(client_id: &str, auth_url: &str, callback_port: u16) -> Self {
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

// =============================================================================
// Helpers
// =============================================================================

/// Build a typed `BasicClient` from a `ClaudeOAuthConfig`.
///
/// Claude uses PKCE without a client secret. We set `AuthType::RequestBody`
/// so `client_id` is included as a body parameter (which the `JsonHttpClient`
/// converts to JSON).
fn build_oauth2_client(config: &ClaudeOAuthConfig) -> Result<OAuthClient, OAuthError> {
    let client_id = ClientId::new(config.client_id.clone());
    let auth_url = AuthUrl::new(config.auth_url.clone())
        .map_err(|e| OAuthError::Other(format!("Invalid auth URL: {}", e)))?;
    let token_url = TokenUrl::new(config.token_url.clone())
        .map_err(|e| OAuthError::Other(format!("Invalid token URL: {}", e)))?;
    let redirect_url = RedirectUrl::new(config.redirect_uri.clone())
        .map_err(|e| OAuthError::Other(format!("Invalid redirect URI: {}", e)))?;

    let client = BasicClient::new(client_id)
        .set_auth_uri(auth_url)
        .set_token_uri(token_url)
        .set_redirect_uri(redirect_url)
        .set_auth_type(AuthType::RequestBody);

    Ok(client)
}

/// HTTP client wrapper that converts form-encoded requests to JSON.
///
/// Claude's token endpoint expects JSON-encoded request bodies, but the
/// `oauth2` crate sends form-encoded by default. This wrapper intercepts
/// requests with `application/x-www-form-urlencoded` content type and
/// re-serializes the body as JSON before executing.
///
/// Wraps a shared `reqwest::Client` (cheap `Arc` clone) to reuse the
/// connection pool and TLS state from `OAuthManager`.
struct JsonHttpClient(reqwest::Client);

/// Convert a form-encoded body (`key1=value1&key2=value2`) to a JSON object.
///
/// Returns `Some(json_bytes)` on success, or `None` if serialization fails
/// (so the caller can keep the original body and Content-Type unchanged).
fn form_to_json(body: &[u8]) -> Option<Vec<u8>> {
    let form_str = String::from_utf8_lossy(body);
    let map: serde_json::Map<String, serde_json::Value> = form_str
        .split('&')
        .filter(|pair| !pair.is_empty())
        .filter_map(|pair| {
            let Some((key, value)) = pair.split_once('=') else {
                warn!(pair = %pair, "Skipping malformed form pair (no '=' separator)");
                return None;
            };
            let key = urlencoding::decode(key)
                .unwrap_or_else(|_| key.into())
                .into_owned();
            let value = urlencoding::decode(value)
                .unwrap_or_else(|_| value.into())
                .into_owned();
            Some((key, serde_json::Value::String(value)))
        })
        .collect();
    match serde_json::to_vec(&map) {
        Ok(json) => Some(json),
        Err(e) => {
            warn!(error = %e, "Failed to serialize form data as JSON, keeping original body");
            None
        }
    }
}

impl<'c> oauth2::AsyncHttpClient<'c> for JsonHttpClient {
    type Error = HttpClientError<reqwest::Error>;
    type Future =
        Pin<Box<dyn Future<Output = Result<oauth2::HttpResponse, Self::Error>> + Send + Sync + 'c>>;

    fn call(&'c self, request: oauth2::HttpRequest) -> Self::Future {
        Box::pin(async move {
            let (mut parts, body) = request.into_parts();

            let is_form = parts
                .headers
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|ct| ct.starts_with("application/x-www-form-urlencoded"));

            // Only replace Content-Type and body when conversion actually succeeds;
            // on failure, keep the original form-encoded body and header intact.
            let body = if is_form {
                if let Some(json) = form_to_json(&body) {
                    parts.headers.insert(
                        reqwest::header::CONTENT_TYPE,
                        reqwest::header::HeaderValue::from_static("application/json"),
                    );
                    json
                } else {
                    body
                }
            } else {
                body
            };

            let request = oauth2::HttpRequest::from_parts(parts, body);

            // Build and execute the request explicitly.
            let response = self
                .0
                .execute(request.try_into().map_err(Box::new)?)
                .await
                .map_err(Box::new)?;

            let mut builder = oauth2::http::Response::builder()
                .status(response.status())
                .version(response.version());

            for (name, value) in response.headers().iter() {
                builder = builder.header(name, value);
            }

            builder
                .body(response.bytes().await.map_err(Box::new)?.to_vec())
                .map_err(HttpClientError::Http)
        })
    }
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

/// Build the Claude authorization URL with PKCE.
///
/// Generates a PKCE challenge/verifier pair internally and returns both the
/// authorization URL and the PKCE verifier secret (which must be stored for
/// the token exchange step).
///
/// Claude requires a special `code=true` parameter.
pub fn build_authorize_url(
    config: &ClaudeOAuthConfig,
    state: &str,
) -> Result<(String, String), OAuthError> {
    let client = build_oauth2_client(config)?;
    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();

    let mut auth_request = client
        .authorize_url(|| CsrfToken::new(state.to_string()))
        .set_pkce_challenge(pkce_challenge)
        .add_extra_param("code", "true");

    for scope in &config.scopes {
        auth_request = auth_request.add_scope(Scope::new(scope.clone()));
    }

    let (url, _csrf_token) = auth_request.url();

    Ok((url.to_string(), pkce_verifier.secret().to_string()))
}

/// Exchange an authorization code for tokens.
///
/// Uses a custom `JsonHttpClient` to convert the oauth2 crate's form-encoded
/// request body to JSON, as required by Claude's token endpoint.
pub async fn exchange_code(
    http_client: &reqwest::Client,
    config: &ClaudeOAuthConfig,
    code: &str,
    verifier: &str,
) -> Result<TokenInfo, OAuthError> {
    debug!("Exchanging authorization code for Claude tokens");

    let client = build_oauth2_client(config)?;
    let http_client = JsonHttpClient(http_client.clone());

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
            OAuthError::ExchangeFailed("No refresh token in response".to_string())
        })?;
    let expires_in = token_response.expires_in().map(|d| d.as_secs() as i64);

    debug!("Claude token exchange successful");

    Ok(TokenInfo::new(
        access_token,
        Some(refresh_token),
        expires_in,
        PROVIDER_ID,
    ))
}

/// Refresh the Claude access token.
///
/// Handles composite token format (`refresh|project_id|managed_project_id`),
/// using only the base refresh token for the request and re-attaching
/// project IDs to the result.
pub async fn refresh_token(
    http_client: &reqwest::Client,
    config: &ClaudeOAuthConfig,
    refresh_token_value: &str,
) -> Result<TokenInfo, OAuthError> {
    // Parse composite token format if present
    let parts: Vec<&str> = refresh_token_value.split('|').collect();
    let base_refresh = parts[0];

    debug!("Refreshing Claude access token");

    let client = build_oauth2_client(config)?;
    let http_client = JsonHttpClient(http_client.clone());

    let token_response = client
        .exchange_refresh_token(&RefreshToken::new(base_refresh.to_string()))
        .request_async(&http_client)
        .await
        .map_err(map_token_error)?;

    debug!("Claude token refresh successful");

    let access_token = token_response.access_token().secret().to_string();
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

    /// Create a `ClaudeOAuthConfig` whose `token_url` points at the given wiremock server.
    fn mock_config(mock_server_uri: &str) -> ClaudeOAuthConfig {
        ClaudeOAuthConfig {
            client_id: "test-client-id".to_string(),
            auth_url: DEFAULT_AUTH_URL.to_string(),
            token_url: mock_server_uri.to_string(),
            redirect_uri: "http://localhost:19284/oauth/callback/claude".to_string(),
            scopes: DEFAULT_SCOPES.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// Shared test HTTP client (reused across tests like `OAuthManager` does).
    fn test_http_client() -> reqwest::Client {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .unwrap()
    }

    /// Standard successful token response JSON.
    fn success_token_json() -> serde_json::Value {
        serde_json::json!({
            "access_token": "claude-test-access-token",
            "token_type": "Bearer",
            "expires_in": 3600,
            "refresh_token": "claude-test-refresh-token"
        })
    }

    /// Successful token response without a refresh token.
    fn success_no_refresh_json() -> serde_json::Value {
        serde_json::json!({
            "access_token": "claude-new-access-token",
            "token_type": "Bearer",
            "expires_in": 3600
        })
    }

    // =========================================================================
    // Sync tests for build_authorize_url and config
    // =========================================================================

    #[test]
    fn test_build_authorize_url_contains_code_true() {
        let config = ClaudeOAuthConfig::new("test-client-id", 19284);
        let (url, _verifier) = build_authorize_url(&config, "test_state").unwrap();

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
    fn test_build_authorize_url_returns_verifier() {
        let config = ClaudeOAuthConfig::new("test-client-id", 19284);
        let (url, verifier) = build_authorize_url(&config, "state").unwrap();

        // Verifier should be non-empty
        assert!(!verifier.is_empty());
        // URL should contain a code_challenge derived from the verifier
        assert!(url.contains("code_challenge="));
    }

    #[test]
    fn test_build_authorize_url_unique_verifiers() {
        let config = ClaudeOAuthConfig::new("test-client-id", 19284);
        let (_url1, verifier1) = build_authorize_url(&config, "state1").unwrap();
        let (_url2, verifier2) = build_authorize_url(&config, "state2").unwrap();

        assert_ne!(verifier1, verifier2);
    }

    #[test]
    fn test_build_authorize_url_starts_with_claude() {
        let config = ClaudeOAuthConfig::new("test-client-id", 19284);
        let (url, _verifier) = build_authorize_url(&config, "state").unwrap();
        assert!(url.starts_with("https://claude.ai/"));
    }

    #[test]
    fn test_config_from_provider() {
        let config =
            ClaudeOAuthConfig::from_provider_config("my-client", "https://custom.auth.url", 8080);
        assert_eq!(config.client_id, "my-client");
        assert_eq!(config.auth_url, "https://custom.auth.url");
        assert_eq!(
            config.redirect_uri,
            "http://localhost:8080/oauth/callback/claude"
        );
        assert!(!config.scopes.is_empty());
    }

    #[test]
    fn test_config_new_uses_defaults() {
        let config = ClaudeOAuthConfig::new("my-client", 19284);
        assert_eq!(config.auth_url, DEFAULT_AUTH_URL);
        assert_eq!(config.token_url, DEFAULT_TOKEN_URL);
    }

    // =========================================================================
    // form_to_json unit tests
    // =========================================================================

    #[test]
    fn test_form_to_json_basic() {
        let body = b"grant_type=authorization_code&code=test-code";
        let json = form_to_json(body).expect("conversion should succeed");
        let parsed: serde_json::Value = serde_json::from_slice(&json).unwrap();
        assert_eq!(parsed["grant_type"], "authorization_code");
        assert_eq!(parsed["code"], "test-code");
    }

    #[test]
    fn test_form_to_json_decodes_percent_encoding() {
        let body = b"redirect_uri=http%3A%2F%2Flocalhost%3A8080%2Fcallback";
        let json = form_to_json(body).expect("conversion should succeed");
        let parsed: serde_json::Value = serde_json::from_slice(&json).unwrap();
        assert_eq!(parsed["redirect_uri"], "http://localhost:8080/callback");
    }

    // =========================================================================
    // Async wiremock tests for exchange_code and refresh_token
    // =========================================================================

    #[tokio::test]
    async fn test_exchange_code_success() {
        let mock_server = wiremock::MockServer::start().await;

        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(success_token_json()),
            )
            .mount(&mock_server)
            .await;

        let config = mock_config(&mock_server.uri());
        let client = test_http_client();
        let result = exchange_code(&client, &config, "test-auth-code", "test-verifier").await;

        let token = result.expect("exchange_code should succeed");
        assert_eq!(token.access_token, "claude-test-access-token");
        assert_eq!(
            token.refresh_token.as_deref(),
            Some("claude-test-refresh-token")
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
    async fn test_exchange_code_sends_json() {
        let mock_server = wiremock::MockServer::start().await;

        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/"))
            .and(wiremock::matchers::header("Content-Type", "application/json"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(success_token_json()),
            )
            .mount(&mock_server)
            .await;

        let config = mock_config(&mock_server.uri());
        let client = test_http_client();
        let result = exchange_code(&client, &config, "test-code", "test-verifier").await;

        // If the mock matched (JSON content-type), the request succeeded
        result.expect("exchange_code should send JSON and succeed");
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
        let result = refresh_token(&client, &config, "original-refresh").await;

        let token = result.expect("refresh should succeed");
        assert_eq!(token.access_token, "claude-new-access-token");
        // Should preserve original when server doesn't return new refresh
        assert_eq!(token.refresh_token.as_deref(), Some("original-refresh"));
        assert!(token.expires_at.is_some());
        assert_eq!(token.provider, PROVIDER_ID);
    }

    #[tokio::test]
    async fn test_refresh_token_returns_new_refresh() {
        let mock_server = wiremock::MockServer::start().await;

        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(success_token_json()),
            )
            .mount(&mock_server)
            .await;

        let config = mock_config(&mock_server.uri());
        let client = test_http_client();
        let result = refresh_token(&client, &config, "old-refresh").await;

        let token = result.expect("refresh should succeed");
        // When server returns a new refresh token, use it
        assert_eq!(
            token.refresh_token.as_deref(),
            Some("claude-test-refresh-token")
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
        let result = refresh_token(&client, &config, "base-refresh|proj-123|managed-456").await;

        let token = result.expect("refresh should succeed");
        assert_eq!(token.access_token, "claude-new-access-token");

        // Project IDs should be preserved in the composite token
        let (base, project, managed) = token.parse_refresh_parts();
        assert_eq!(base, "base-refresh");
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
        let result = refresh_token(&client, &config, "base-refresh|proj-only").await;

        let token = result.expect("refresh should succeed");
        let (base, project, managed) = token.parse_refresh_parts();
        assert_eq!(base, "base-refresh");
        assert_eq!(project.as_deref(), Some("proj-only"));
        assert!(managed.is_none());
    }
}
