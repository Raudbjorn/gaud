//! Raw Kiro API access (escape hatch).
//!
//! Use this when you need to send payloads directly in Kiro's native format,
//! bypassing the Anthropic Messages API abstraction.

use crate::error::Result;

/// Send a raw Kiro payload and get the full response text.
pub async fn raw_request(
    http: &crate::transport::http::KiroHttpClient,
    region: &str,
    profile_arn: Option<&str>,
    payload: &serde_json::Value,
) -> Result<String> {
    let url = crate::config::generate_assistant_response_url(region, profile_arn)?;

    let response = http.post_with_retry(&url, payload).await?;
    let text = response
        .text()
        .await
        .map_err(|e| crate::error::Error::Stream(format!("Failed to read response: {}", e)))?;

    Ok(text)
}

/// Send a raw Kiro payload and get a streaming response.
pub async fn raw_request_stream(
    http: &crate::transport::http::KiroHttpClient,
    region: &str,
    profile_arn: Option<&str>,
    payload: &serde_json::Value,
) -> Result<reqwest::Response> {
    let url = crate::config::generate_assistant_response_url(region, profile_arn)?;
    http.post_streaming(&url, payload).await
}
