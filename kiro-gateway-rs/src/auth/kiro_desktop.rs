//! Kiro Desktop Auth token refresh.

use tracing::{debug, info};

use crate::config::kiro_refresh_url;
use crate::error::{Error, Result};
use crate::models::auth::{KiroDesktopRefreshResponse, KiroTokenInfo};

/// Refresh token via Kiro Desktop Auth endpoint.
///
/// POST `https://prod.{region}.auth.desktop.kiro.dev/refreshToken`
/// Body: `{"refreshToken": "..."}`
pub async fn refresh_token(
    client: &reqwest::Client,
    token_info: &KiroTokenInfo,
    fingerprint: &str,
) -> Result<KiroDesktopRefreshResponse> {
    if token_info.refresh_token.is_empty() {
        return Err(Error::MissingCredential("refresh_token".into()));
    }

    let url = kiro_refresh_url(&token_info.region);
    info!("Refreshing token via Kiro Desktop Auth...");

    let payload = serde_json::json!({
        "refreshToken": token_info.refresh_token,
    });

    let response = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header(
            "User-Agent",
            format!("{}-{}", super::constants::KIRO_IDE_VERSION, fingerprint),
        )
        .json(&payload)
        .send()
        .await
        .map_err(|e| Error::RefreshFailed(format!("Kiro Desktop request failed: {}", e)))?;

    if !response.status().is_success() {
        let status = response.status().as_u16();
        let body = response.text().await.unwrap_or_default();
        return Err(Error::RefreshFailed(format!(
            "Kiro Desktop Auth returned {}: {}",
            status, body
        )));
    }

    let data: KiroDesktopRefreshResponse = response
        .json()
        .await
        .map_err(|e| Error::RefreshFailed(format!("Failed to parse refresh response: {}", e)))?;

    if data.access_token.is_empty() {
        return Err(Error::RefreshFailed(
            "Response does not contain accessToken".into(),
        ));
    }

    debug!("Token refreshed via Kiro Desktop Auth");
    Ok(data)
}
