//! GitHub Copilot Device Code flow (RFC 8628).
//!
//! Implements authentication for GitHub Copilot using the OAuth 2.0
//! Device Authorization Grant. Unlike PKCE-based flows (Claude, Gemini),
//! Device Code flow works without a browser redirect:
//!
//! 1. Application requests a device code from GitHub
//! 2. User visits a verification URL and enters the displayed code
//! 3. Application polls GitHub until the user completes authorization
//! 4. Access token is received
//!
//! # Endpoints
//! - Device code: `https://github.com/login/device/code`
//! - Token: `https://github.com/login/oauth/access_token`
//! - Client ID: `Iv1.b507a08c87ecfe98`

use serde::Deserialize;
use tracing::{debug, info};

use super::OAuthError;
use super::token::TokenInfo;

/// Provider identifier for Copilot.
pub const PROVIDER_ID: &str = "copilot";

/// Default GitHub Device Code endpoint.
const DEVICE_CODE_URL: &str = "https://github.com/login/device/code";

/// Default GitHub Token endpoint.
const TOKEN_URL: &str = "https://github.com/login/oauth/access_token";

/// Default GitHub OAuth Client ID for Copilot.
const DEFAULT_CLIENT_ID: &str = "Iv1.b507a08c87ecfe98";

/// Configuration for the Copilot Device Code flow.
#[derive(Debug, Clone)]
pub struct CopilotOAuthConfig {
    pub client_id: String,
    pub device_code_url: String,
    pub token_url: String,
}

impl CopilotOAuthConfig {
    /// Create config from the application config.
    pub fn from_provider_config(client_id: &str) -> Self {
        Self {
            client_id: client_id.to_string(),
            device_code_url: DEVICE_CODE_URL.to_string(),
            token_url: TOKEN_URL.to_string(),
        }
    }

    /// Create config with the default client ID.
    pub fn new() -> Self {
        Self::from_provider_config(DEFAULT_CLIENT_ID)
    }
}

impl Default for CopilotOAuthConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// Response from the device code request.
#[derive(Debug, Clone, Deserialize)]
pub struct DeviceCodeResponse {
    /// The device code used for polling.
    pub device_code: String,
    /// The code the user must enter at the verification URL.
    pub user_code: String,
    /// The URL where the user enters the code.
    pub verification_uri: String,
    /// Seconds until the device code expires.
    pub expires_in: u64,
    /// Minimum seconds between poll attempts.
    pub interval: u64,
}

/// Response from the token polling endpoint.
#[derive(Deserialize)]
struct TokenPollResponse {
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    token_type: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    error_description: Option<String>,
}

/// Result of a single poll attempt.
#[derive(Debug)]
pub enum PollResult {
    /// User hasn't completed authorization yet, keep polling.
    Pending,
    /// Server asked us to slow down, increase interval.
    SlowDown,
    /// Authorization complete, contains the access token.
    Complete(String),
}

/// Request a device code from GitHub.
///
/// This starts the device code flow. The caller should display the
/// `user_code` and `verification_uri` to the user.
pub async fn request_device_code(
    http_client: &reqwest::Client,
    config: &CopilotOAuthConfig,
) -> Result<DeviceCodeResponse, OAuthError> {
    info!("Requesting GitHub device code for Copilot");

    let scope = "read:user".to_string();
    let response = http_client
        .post(&config.device_code_url)
        .header("Accept", "application/json")
        .form(&[
            ("client_id", &config.client_id),
            ("scope", "read:user"),
        ])
        .send()
        .await?;

    let status = response.status();
    let body = response.text().await?;

    if !status.is_success() {
        return Err(OAuthError::ExchangeFailed(format!(
            "Device code request failed (HTTP {}): {}",
            status.as_u16(),
            body
        )));
    }

    let device_response: DeviceCodeResponse = serde_json::from_str(&body).map_err(|e| {
        OAuthError::ExchangeFailed(format!("Failed to parse device code response: {}", e))
    })?;

    debug!(
        user_code = %device_response.user_code,
        verification_uri = %device_response.verification_uri,
        expires_in = device_response.expires_in,
        interval = device_response.interval,
        "Device code obtained"
    );

    Ok(device_response)
}

/// Poll the token endpoint once.
///
/// Returns `PollResult::Pending` if the user hasn't authorized yet,
/// `PollResult::SlowDown` if we need to increase the interval,
/// or `PollResult::Complete(token)` on success.
pub async fn poll_for_token(
    http_client: &reqwest::Client,
    config: &CopilotOAuthConfig,
    device_code: &str,
) -> Result<PollResult, OAuthError> {
    let response = http_client
        .post(&config.token_url)
        .header("Accept", "application/json")
        .form(&[
            ("client_id", config.client_id.as_str()),
            ("device_code", device_code),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ])
        .send()
        .await?;

    let status = response.status();
    let body = response.text().await?;

    let poll_response: TokenPollResponse = serde_json::from_str(&body).map_err(|e| {
        OAuthError::ExchangeFailed(format!("Failed to parse poll response: {}", e))
    })?;

    // Check for access token first (success)
    if let Some(token) = poll_response.access_token {
        return Ok(PollResult::Complete(token));
    }

    // Check error conditions
    match poll_response.error.as_deref() {
        Some("authorization_pending") => Ok(PollResult::Pending),
        Some("slow_down") => Ok(PollResult::SlowDown),
        Some("expired_token") => Err(OAuthError::FlowExpired),
        Some("access_denied") => Err(OAuthError::ExchangeFailed(
            "User denied authorization".to_string(),
        )),
        Some(error) => Err(OAuthError::ExchangeFailed(format!(
            "Poll error: {} - {}",
            error,
            poll_response.error_description.unwrap_or_default()
        ))),
        None => Err(OAuthError::ExchangeFailed(format!(
            "Unexpected poll response (HTTP {}): {}",
            status.as_u16(),
            body
        ))),
    }
}

/// Poll until the device flow completes or times out.
///
/// Handles the complete polling loop, respecting the server-specified
/// interval and backing off on `slow_down` responses.
///
/// `on_pending` is called on each pending poll with the attempt number.
pub async fn poll_until_complete(
    http_client: &reqwest::Client,
    config: &CopilotOAuthConfig,
    device_response: &DeviceCodeResponse,
    mut on_pending: Option<&mut dyn FnMut(u32)>,
) -> Result<String, OAuthError> {
    let mut interval = std::time::Duration::from_secs(device_response.interval.max(5));
    let deadline = tokio::time::Instant::now()
        + std::time::Duration::from_secs(device_response.expires_in);
    let mut attempt: u32 = 0;

    loop {
        if tokio::time::Instant::now() >= deadline {
            return Err(OAuthError::FlowExpired);
        }

        tokio::time::sleep(interval).await;
        attempt += 1;

        if let Some(ref mut cb) = on_pending {
            cb(attempt);
        }

        match poll_for_token(http_client, config, &device_response.device_code).await? {
            PollResult::Complete(token) => {
                info!("Copilot device flow completed successfully");
                return Ok(token);
            }
            PollResult::Pending => {
                debug!(attempt, "Device flow pending, continuing to poll");
            }
            PollResult::SlowDown => {
                interval += std::time::Duration::from_secs(5);
                debug!(?interval, "Slowing down poll interval");
            }
        }
    }
}

/// Create a TokenInfo from a Copilot access token.
///
/// Copilot tokens don't have a traditional refresh token or expiry
/// from the device code flow. The token itself is long-lived.
pub fn create_token_info(access_token: &str) -> TokenInfo {
    TokenInfo::new(
        access_token.to_string(),
        None, // No refresh token in device code flow
        None, // GitHub tokens don't have a set expiry from device flow
        PROVIDER_ID,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = CopilotOAuthConfig::new();
        assert_eq!(config.client_id, DEFAULT_CLIENT_ID);
        assert!(config.device_code_url.contains("github.com"));
        assert!(config.token_url.contains("github.com"));
    }

    #[test]
    fn test_config_from_provider() {
        let config = CopilotOAuthConfig::from_provider_config("custom-client-id");
        assert_eq!(config.client_id, "custom-client-id");
    }

    #[test]
    fn test_create_token_info() {
        let token = create_token_info("gho_test_token");
        assert_eq!(token.access_token, "gho_test_token");
        assert!(token.refresh_token.is_none());
        assert!(token.expires_at.is_none());
        assert_eq!(token.provider, "copilot");
        assert!(!token.is_expired());
    }

    #[test]
    fn test_config_default_impl() {
        let config = CopilotOAuthConfig::default();
        assert_eq!(config.client_id, DEFAULT_CLIENT_ID);
    }
}
