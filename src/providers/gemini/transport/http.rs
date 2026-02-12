//! HTTP client wrapper for Cloud Code API.
//!
//! This module provides an HTTP client that handles:
//! - Request building with appropriate headers
//! - Cloud Code request wrapping format
//! - Endpoint fallback on failure
//! - Error mapping to custom types

use std::time::Duration;

use reqwest::{Response, StatusCode};
use crate::net::{HttpClient as BaseHttpClient, HttpClientBuilder as BaseHttpClientBuilder};
use serde::Serialize;
use tracing::{debug, instrument, warn};

use crate::providers::gemini::constants::{
    CLIENT_METADATA, CLOUDCODE_ENDPOINT_FALLBACKS, GOOG_API_CLIENT, ModelFamily, get_model_family, is_thinking_model,
    USER_AGENT,
};
use crate::providers::gemini::error::{Error, Result};
use crate::providers::gemini::models::google::CloudCodeWrapper;

/// HTTP client wrapper for Cloud Code API requests.
///
/// Handles request building, header construction, and endpoint fallback.
///
/// # Example
///
/// ```rust,ignore
/// use gaud::providers::gemini::transport::HttpClient;
///
/// let client = HttpClient::new();
/// let response = client.post(&url, "token", &body).await?;
/// ```
#[derive(Debug, Clone)]
pub struct HttpClient {
    inner: BaseHttpClient,
    /// Custom base URL for testing.
    base_url: Option<String>,
}

impl HttpClient {
    /// Create a new HTTP client with default settings.
    pub fn new() -> Self {
        Self::builder().build()
    }

    /// Create a builder for constructing a custom HTTP client.
    pub fn builder() -> HttpClientBuilder {
        HttpClientBuilder::default()
    }

    /// Build standard headers for Cloud Code API requests.
    ///
    /// # Arguments
    ///
    /// * `token` - OAuth access token
    /// * `model` - Model name (used to determine model-specific headers)
    /// * `streaming` - Whether this is a streaming request
    ///
    /// # Headers Included
    ///
    /// - `Authorization: Bearer {token}`
    /// - `Content-Type: application/json`
    /// - `User-Agent: {USER_AGENT}`
    /// - `X-Goog-Api-Client: {GOOG_API_CLIENT}`
    /// - `Client-Metadata: {CLIENT_METADATA}`
    /// - `anthropic-beta: interleaved-thinking-2025-05-14` (for Claude thinking models)
    pub fn build_headers(token: &str, model: &str, streaming: bool) -> reqwest::header::HeaderMap {
        let mut headers = reqwest::header::HeaderMap::new();

        // Authorization
        headers.insert(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {}", token).parse().unwrap(),
        );

        // Content type
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            "application/json".parse().unwrap(),
        );

        // User-Agent
        headers.insert(reqwest::header::USER_AGENT, USER_AGENT.parse().unwrap());

        // Google-specific headers
        headers.insert(
            reqwest::header::HeaderName::from_static("x-goog-api-client"),
            GOOG_API_CLIENT.parse().unwrap(),
        );

        headers.insert(
            reqwest::header::HeaderName::from_static("client-metadata"),
            CLIENT_METADATA.parse().unwrap(),
        );

        // Model-specific headers
        let model_family = get_model_family(model);
        if model_family == ModelFamily::Claude && is_thinking_model(model) {
            headers.insert(
                reqwest::header::HeaderName::from_static("anthropic-beta"),
                "interleaved-thinking-2025-05-14".parse().unwrap(),
            );
        }

        // Accept header for streaming
        if streaming {
            headers.insert(
                reqwest::header::ACCEPT,
                "text/event-stream".parse().unwrap(),
            );
        }

        headers
    }

    /// Make a POST request with automatic endpoint fallback.
    ///
    /// Tries each endpoint in the fallback list until one succeeds or
    /// returns a non-retryable error.
    ///
    /// # Arguments
    ///
    /// * `path` - API path (e.g., `/v1internal/generate_content`)
    /// * `token` - OAuth access token
    /// * `body` - Request body to serialize as JSON
    /// * `model` - Model name for headers
    /// * `streaming` - Whether this is a streaming request
    ///
    /// # Returns
    ///
    /// The HTTP response on success.
    ///
    /// # Errors
    ///
    /// Returns an error if all endpoints fail or return non-retryable errors.
    #[instrument(skip(self, token, body), fields(path = %path))]
    pub async fn post_with_fallback<T: Serialize + ?Sized>(
        &self,
        path: &str,
        token: &str,
        body: &T,
        model: &str,
        streaming: bool,
    ) -> Result<Response> {
        let endpoints = self
            .base_url
            .as_ref()
            .map(|url| vec![url.as_str()])
            .unwrap_or_else(|| CLOUDCODE_ENDPOINT_FALLBACKS.to_vec());

        let headers = Self::build_headers(token, model, streaming);
        let mut last_error: Option<Error> = None;

        for (idx, endpoint) in endpoints.iter().enumerate() {
            let url = format!("{}{}", endpoint, path);
            debug!(endpoint = %endpoint, attempt = idx + 1, "Trying endpoint");

            match self.send_request(&url, &headers, body).await {
                Ok(response) => {
                    let status = response.status();

                    // Check for retryable errors (403, 404) for fallback
                    if (status == StatusCode::FORBIDDEN || status == StatusCode::NOT_FOUND)
                        && idx < endpoints.len() - 1
                    {
                        debug!(
                            status = %status,
                            endpoint = %endpoint,
                            "Endpoint returned {} error, trying fallback",
                            status.as_u16()
                        );
                        last_error = Some(Error::api(
                            status.as_u16(),
                            format!("Endpoint returned {}", status),
                            None,
                        ));
                        continue;
                    }

                    // Return response (success or non-retryable error)
                    return Ok(response);
                }
                Err(e) => {
                    warn!(
                        endpoint = %endpoint,
                        error = %e,
                        "Request failed"
                    );

                    // Check if error is retryable (network errors)
                    if is_retryable_error(&e) && idx < endpoints.len() - 1 {
                        last_error = Some(e);
                        continue;
                    }

                    return Err(e);
                }
            }
        }

        // All endpoints failed
        Err(last_error.unwrap_or_else(|| Error::config("No endpoints available")))
    }

    /// Send a single POST request.
    async fn send_request<T: Serialize + ?Sized>(
        &self,
        url: &str,
        headers: &reqwest::header::HeaderMap,
        body: &T,
    ) -> Result<Response> {
        let response = self
            .inner
            .inner()
            .post(url)
            .headers(headers.clone())
            .json(body)
            .send()
            .await?;

        Ok(response)
    }

    /// Get the inner reqwest client.
    pub fn inner(&self) -> &reqwest::Client {
        self.inner.inner()
    }
}

impl Default for HttpClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Check if an error is retryable (network-related).
fn is_retryable_error(error: &Error) -> bool {
    match error {
        Error::Network(e) => e.is_connect() || e.is_timeout() || e.is_request(),
        _ => false,
    }
}

/// Builder for constructing an [`HttpClient`].
///
/// # Example
///
/// ```rust
/// use gaud::providers::gemini::transport::HttpClientBuilder;
/// use std::time::Duration;
///
/// let client = HttpClientBuilder::default()
///     .connect_timeout(Duration::from_secs(30))
///     .request_timeout(Duration::from_secs(300))
///     .build();
/// ```
pub struct HttpClientBuilder {
    base_builder: BaseHttpClientBuilder,
    base_url: Option<String>,
}

impl HttpClientBuilder {
    /// Create a new builder with default settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the connection timeout.
    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.base_builder = self.base_builder.connect_timeout(timeout);
        self
    }

    /// Set the request timeout.
    pub fn request_timeout(mut self, timeout: Duration) -> Self {
        self.base_builder = self.base_builder.request_timeout(timeout);
        self
    }

    /// Set a custom base URL (useful for testing).
    pub fn base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = Some(url.into());
        self
    }

    /// Build the HTTP client.
    pub fn build(self) -> HttpClient {
        HttpClient {
            inner: self.base_builder.build(),
            base_url: self.base_url,
        }
    }
}

impl Default for HttpClientBuilder {
    fn default() -> Self {
        Self {
            base_builder: BaseHttpClientBuilder::default(),
            base_url: None,
        }
    }
}

/// Wrap a request in Cloud Code format.
///
/// Cloud Code API expects requests in a specific wrapper format with
/// project, model, and metadata fields.
///
/// # Arguments
///
/// * `project_id` - The Cloud Code project ID
/// * `model` - The model name
/// * `request` - The inner request to wrap
///
/// # Returns
///
/// A `CloudCodeWrapper` ready for serialization.
#[allow(dead_code)]
pub(crate) fn wrap_request(
    project_id: &str,
    model: &str,
    request: crate::providers::gemini::models::google::GoogleRequest,
) -> CloudCodeWrapper {
    CloudCodeWrapper::new(project_id, model, request).with_request_id(generate_request_id())
}

/// Generate a unique request ID.
///
/// Format: `agent-{uuid}`
pub fn generate_request_id() -> String {
    format!("agent-{}", uuid::Uuid::new_v4())
}

/// Mask a token for safe logging.
///
/// Shows first 4 and last 4 characters, masks the rest.
///
/// # Example
///
/// ```
/// use gaud::providers::gemini::transport::http::mask_token;
///
/// let masked = mask_token("ya29.very_long_access_token_here");
/// assert!(masked.starts_with("ya29"));
/// assert!(masked.ends_with("here"));
/// assert!(masked.contains("***"));
/// ```
pub fn mask_token(token: &str) -> String {
    if token.len() <= 12 {
        return "***".to_string();
    }
    format!("{}***{}", &token[..4], &token[token.len() - 4..])
}

/// Build the API path for generate content requests.
///
/// # Arguments
///
/// * `streaming` - Whether this is a streaming request
///
/// # Returns
///
/// The API path string.
pub fn build_api_path(streaming: bool) -> &'static str {
    if streaming {
        crate::providers::gemini::constants::API_PATH_STREAM_GENERATE_CONTENT
    } else {
        crate::providers::gemini::constants::API_PATH_GENERATE_CONTENT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_headers_basic() {
        let headers = HttpClient::build_headers("test_token", "claude-sonnet-4-5", false);

        assert!(headers.contains_key(reqwest::header::AUTHORIZATION));
        assert!(headers.contains_key(reqwest::header::CONTENT_TYPE));
        assert!(headers.contains_key(reqwest::header::USER_AGENT));
        assert!(headers.contains_key("x-goog-api-client"));
        assert!(headers.contains_key("client-metadata"));

        // Non-thinking model should not have anthropic-beta
        assert!(!headers.contains_key("anthropic-beta"));
    }

    #[test]
    fn test_build_headers_claude_thinking() {
        let headers = HttpClient::build_headers("test_token", "claude-sonnet-4-5-thinking", false);

        assert!(headers.contains_key("anthropic-beta"));
        assert_eq!(
            headers.get("anthropic-beta").unwrap(),
            "interleaved-thinking-2025-05-14"
        );
    }

    #[test]
    fn test_build_headers_gemini_no_anthropic_beta() {
        let headers = HttpClient::build_headers("test_token", "gemini-3-flash", false);

        // Gemini models should not have anthropic-beta
        assert!(!headers.contains_key("anthropic-beta"));
    }

    #[test]
    fn test_build_headers_streaming() {
        let headers = HttpClient::build_headers("test_token", "claude-sonnet-4-5", true);

        assert!(headers.contains_key(reqwest::header::ACCEPT));
        assert_eq!(
            headers.get(reqwest::header::ACCEPT).unwrap(),
            "text/event-stream"
        );
    }

    #[test]
    fn test_mask_token() {
        // Long token
        let masked = mask_token("ya29.very_long_access_token_here_xyz");
        assert!(masked.starts_with("ya29"));
        assert!(masked.ends_with("_xyz"));
        assert!(masked.contains("***"));

        // Short token
        let masked = mask_token("short");
        assert_eq!(masked, "***");

        // Empty token
        let masked = mask_token("");
        assert_eq!(masked, "***");
    }

    #[test]
    fn test_generate_request_id() {
        let id1 = generate_request_id();
        let id2 = generate_request_id();

        assert!(id1.starts_with("agent-"));
        assert!(id2.starts_with("agent-"));
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_build_api_path() {
        assert_eq!(build_api_path(false), "/v1internal:generateContent");
        assert_eq!(
            build_api_path(true),
            "/v1internal:streamGenerateContent?alt=sse"
        );
    }

    #[test]
    fn test_http_client_builder_default() {
        let client = HttpClientBuilder::default().build();
        assert!(client.base_url.is_none());
    }

    #[test]
    fn test_http_client_builder_custom() {
        let client = HttpClientBuilder::new()
            .connect_timeout(Duration::from_secs(60))
            .request_timeout(Duration::from_secs(600))
            .base_url("https://test.example.com")
            .build();

        assert_eq!(
            client.base_url,
            Some("https://test.example.com".to_string())
        );
    }

    #[test]
    fn test_wrap_request() {
        use crate::providers::gemini::models::google::{Content, GoogleRequest, Part};

        let request = GoogleRequest::with_contents(vec![Content::user(vec![Part::text("Hello")])]);

        let wrapped = wrap_request("project-123", "claude-sonnet-4-5", request);

        assert_eq!(wrapped.project, "project-123");
        assert_eq!(wrapped.model, "claude-sonnet-4-5");
        assert!(wrapped.request_id.is_some());
        assert!(wrapped.request_id.as_ref().unwrap().starts_with("agent-"));
        assert_eq!(wrapped.user_agent, Some("antigravity".to_string()));
        assert_eq!(wrapped.request_type, Some("agent".to_string()));
    }

    #[test]
    fn test_is_retryable_error() {
        // API errors are not retryable by default
        let api_error = Error::api(500, "Server error", None);
        assert!(!is_retryable_error(&api_error));

        // Config errors are not retryable
        let config_error = Error::config("Missing field");
        assert!(!is_retryable_error(&config_error));
    }
}
