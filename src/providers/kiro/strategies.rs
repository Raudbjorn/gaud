use async_trait::async_trait;
use chrono::Utc;
use reqwest::Client;
use serde::Deserialize;
use tracing::debug;

use crate::providers::ProviderError;
use super::models::{AuthType, KiroTokenInfo, TokenUpdate};

#[async_trait]
pub trait AuthStrategy: Send + Sync {
    /// Refresh the token using this strategy
    async fn refresh(&self, current_token: &KiroTokenInfo, http: &Client) -> Result<TokenUpdate, ProviderError>;

    /// Check if this strategy handles the given auth type
    fn can_handle(&self, auth_type: AuthType) -> bool;
}

pub struct KiroDesktopStrategy {
    fingerprint: String,
}

impl KiroDesktopStrategy {
    pub fn new(fingerprint: String) -> Self {
        Self { fingerprint }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KiroDesktopRefreshResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: i64,
    profile_arn: Option<String>,
}

#[async_trait]
impl AuthStrategy for KiroDesktopStrategy {
    fn can_handle(&self, auth_type: AuthType) -> bool {
        auth_type == AuthType::KiroDesktop
    }

    async fn refresh(&self, info: &KiroTokenInfo, http: &Client) -> Result<TokenUpdate, ProviderError> {
        debug!("Refreshing via Kiro Desktop Auth");
        let url = format!("https://prod.{}.auth.desktop.kiro.dev/refreshToken", info.region);
        let payload = serde_json::json!({ "refreshToken": info.refresh_token });
        let ua = format!("KiroIDE-0.7.45-{}", self.fingerprint);

        let resp = http.post(&url)
            .header("user-agent", &ua)
            .json(&payload)
            .send().await
            .map_err(|e| ProviderError::Other(format!("Kiro refresh error: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(ProviderError::NoToken { provider: format!("kiro: refresh failed {}: {}", status, body) });
        }

        let data: KiroDesktopRefreshResponse = resp.json().await
            .map_err(|e| ProviderError::Other(format!("Kiro parse error: {e}")))?;

        Ok(TokenUpdate {
            access_token: data.access_token,
            refresh_token: data.refresh_token,
            expires_at: Utc::now().timestamp() + data.expires_in,
            profile_arn: data.profile_arn,
        })
    }
}

pub struct AwsSsoOidcStrategy;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AwsSsoOidcRefreshResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: i64,
}

#[async_trait]
impl AuthStrategy for AwsSsoOidcStrategy {
    fn can_handle(&self, auth_type: AuthType) -> bool {
        auth_type == AuthType::AwsSsoOidc
    }

    async fn refresh(&self, info: &KiroTokenInfo, http: &Client) -> Result<TokenUpdate, ProviderError> {
        debug!("Refreshing via AWS SSO OIDC");
        let sso_region = info.sso_region.as_deref().unwrap_or(&info.region);
        let url = format!("https://oidc.{}.amazonaws.com/token", sso_region);

        let client_id = info.client_id.as_ref().ok_or_else(|| ProviderError::Other("Missing client_id for SSO refresh".into()))?;
        let client_secret = info.client_secret.as_ref().ok_or_else(|| ProviderError::Other("Missing client_secret for SSO refresh".into()))?;

        let payload = serde_json::json!({
            "grantType": "refresh_token",
            "clientId": client_id,
            "clientSecret": client_secret,
            "refreshToken": info.refresh_token,
        });

        let resp = http.post(&url).json(&payload).send().await
            .map_err(|e| ProviderError::Other(format!("AWS SSO OIDC refresh error: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(ProviderError::NoToken { provider: format!("aws_sso: refresh failed {}: {}", status, body) });
        }

        let data: AwsSsoOidcRefreshResponse = resp.json().await
            .map_err(|e| ProviderError::Other(format!("AWS SSO OIDC parse error: {e}")))?;

        Ok(TokenUpdate {
            access_token: data.access_token,
            refresh_token: data.refresh_token,
            expires_at: Utc::now().timestamp() + data.expires_in,
            profile_arn: None, // SSO doesn't return profile ARN in refresh
        })
    }
}
