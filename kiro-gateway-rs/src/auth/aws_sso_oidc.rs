//! AWS SSO OIDC token refresh.

use tracing::{debug, info};

use crate::config::aws_sso_oidc_url;
use crate::error::{Error, Result};
use crate::models::auth::{AwsSsoOidcRefreshResponse, KiroTokenInfo};

/// Refresh token via AWS SSO OIDC endpoint.
///
/// POST `https://oidc.{region}.amazonaws.com/token`
/// Body (JSON): `{"grantType": "refresh_token", "clientId": "...", "clientSecret": "...", "refreshToken": "..."}`
pub async fn refresh_token(
    client: &reqwest::Client,
    token_info: &KiroTokenInfo,
) -> Result<AwsSsoOidcRefreshResponse> {
    if token_info.refresh_token.is_empty() {
        return Err(Error::MissingCredential("refresh_token".into()));
    }
    let client_id = token_info
        .client_id
        .as_deref()
        .ok_or_else(|| Error::MissingCredential("client_id (required for AWS SSO OIDC)".into()))?;
    let client_secret = token_info.client_secret.as_deref().ok_or_else(|| {
        Error::MissingCredential("client_secret (required for AWS SSO OIDC)".into())
    })?;

    // Use SSO region for OIDC endpoint (may differ from API region)
    let sso_region = token_info
        .sso_region
        .as_deref()
        .unwrap_or(&token_info.region);
    let url = aws_sso_oidc_url(sso_region);

    info!("Refreshing token via AWS SSO OIDC (region: {})...", sso_region);

    // AWS SSO OIDC CreateToken API uses JSON with camelCase parameters
    let payload = serde_json::json!({
        "grantType": "refresh_token",
        "clientId": client_id,
        "clientSecret": client_secret,
        "refreshToken": token_info.refresh_token,
    });

    let response = client
        .post(&url)
        .header("Content-Type", "application/json")
        .json(&payload)
        .send()
        .await
        .map_err(|e| Error::RefreshFailed(format!("AWS SSO OIDC request failed: {}", e)))?;

    if !response.status().is_success() {
        let status = response.status().as_u16();
        let body = response.text().await.unwrap_or_default();
        return Err(Error::RefreshFailed(format!(
            "AWS SSO OIDC returned {}: {}",
            status, body
        )));
    }

    let data: AwsSsoOidcRefreshResponse = response
        .json()
        .await
        .map_err(|e| Error::RefreshFailed(format!("Failed to parse OIDC response: {}", e)))?;

    if data.access_token.is_empty() {
        return Err(Error::RefreshFailed(
            "OIDC response does not contain accessToken".into(),
        ));
    }

    debug!("Token refreshed via AWS SSO OIDC");
    Ok(data)
}
