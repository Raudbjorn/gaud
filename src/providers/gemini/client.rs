//! Cloud Code API client.
//!
//! This module provides the main client for interacting with the Cloud Code API.
//! It handles authentication via the `TokenProvider` trait.

use std::pin::Pin;
use std::sync::Arc;

use futures::Stream;
use reqwest::StatusCode;
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;
use tracing::{debug, info, instrument};

use crate::auth::TokenProvider;
use crate::providers::gemini::constants::{
    ANTIGRAVITY_SYSTEM_INSTRUCTION, API_PATH_GENERATE_CONTENT, API_PATH_STREAM_GENERATE_CONTENT,
};

use crate::providers::gemini::discovery::discover_project;
use crate::providers::gemini::error::{AuthError, Error, Result};
use crate::providers::gemini::models::google::{CloudCodeWrapper, Content, GoogleRequest, Part};
use crate::providers::gemini::models::stream::StreamEvent;
use crate::providers::gemini::transport::http::{HttpClient, generate_request_id};
use crate::providers::gemini::transport::sse::SseStream;

use crate::providers::gemini::thinking::SignatureCache;

/// Cloud Code API client.
///
/// Thread-safe client for Google Cloud Code API.
#[derive(Clone)]
pub struct CloudCodeClient {
    /// Token provider for authentication.
    token_provider: Arc<dyn TokenProvider>,
    /// HTTP client for API requests.
    http: HttpClient,
    /// Cached project ID.
    project_id: Arc<RwLock<Option<String>>>,
    /// Cached managed project ID.
    managed_project_id: Arc<RwLock<Option<String>>>,
    /// Signature cache for thinking signatures.
    signature_cache: Arc<SignatureCache>,
}

impl CloudCodeClient {
    /// Create a new client builder.
    pub fn builder() -> CloudCodeClientBuilder {
        CloudCodeClientBuilder::default()
    }

    /// Create a new client with the given token provider.
    pub fn new(token_provider: Arc<dyn TokenProvider>) -> Self {
        Self::builder().with_token_provider(token_provider).build()
    }

    /// check if authenticated
    pub async fn is_authenticated(&self) -> Result<bool> {
        // Attempt to get a token to verify authentication status
        match self.token_provider.get_token("gemini").await {
            Ok(_) => Ok(true),
            Err(_) => Ok(false),
        }
    }

    /// Get the current access token.
    pub async fn get_access_token(&self) -> Result<String> {
        self.token_provider
            .get_token("gemini")
            .await
            .map_err(|e| Error::Auth(AuthError::Other(e.to_string())))
    }

    /// Get the project ID, discovering it if necessary.
    ///
    /// The project ID is cached after the first discovery.
    #[instrument(skip(self))]
    pub async fn get_project_id(&self) -> Result<String> {
        // Check cache first
        {
            let cached = self.project_id.read().await;
            if let Some(id) = cached.as_ref() {
                return Ok(id.clone());
            }
        }

        // Discover project
        let token = self.get_access_token().await?;
        let project_info = discover_project(&token, None).await?;

        // Cache the results
        {
            let mut project_id = self.project_id.write().await;
            *project_id = Some(project_info.project_id.clone());
        }

        if let Some(managed) = &project_info.managed_project_id {
            let mut managed_id = self.managed_project_id.write().await;
            *managed_id = Some(managed.clone());
        }

        info!(
            project_id = %project_info.project_id,
            tier = %project_info.subscription_tier,
            "Discovered project"
        );

        Ok(project_info.project_id)
    }

    /// Send a Google format request (non-streaming).
    #[instrument(skip(self, request), fields(model = %model))]
    pub async fn request(
        &self,
        model: &str,
        request: GoogleRequest,
    ) -> Result<crate::providers::gemini::models::google::GoogleResponse> {
        let token = self.get_access_token().await?;
        let project_id = self.get_project_id().await?;

        debug!(
            model = %model,
            "Sending request"
        );

        // Wrap in Cloud Code format
        let wrapped = self.wrap_request(&project_id, model, request);

        let path = API_PATH_GENERATE_CONTENT;

        let response = self
            .http
            .post_with_fallback(path, &token, &wrapped, model, false)
            .await?;

        let status = response.status();
        self.handle_response_status(status, &response).await?;

        // Parse response
        let google_response: crate::providers::gemini::models::google::GoogleResponse =
            response.json().await?;

        debug!("Request completed");

        Ok(google_response)
    }

    /// Send a request and return a stream of events.
    #[instrument(skip(self, request), fields(model = %model))]
    pub async fn request_stream(
        &self,
        model: &str,
        request: GoogleRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        let token = self.get_access_token().await?;
        let project_id = self.get_project_id().await?;

        debug!(
            model = %model,
            "Starting streaming request"
        );

        // Wrap in Cloud Code format
        let wrapped = self.wrap_request(&project_id, model, request);

        // Send streaming request
        let path = API_PATH_STREAM_GENERATE_CONTENT;
        let response = self
            .http
            .post_with_fallback(path, &token, &wrapped, model, true)
            .await?;

        let status = response.status();
        self.handle_response_status(status, &response).await?;

        // Create SSE stream
        let byte_stream = response.bytes_stream();
        let sse_stream = SseStream::new(byte_stream, model);

        Ok(Box::pin(sse_stream))
    }

    /// Handle response status codes.
    async fn handle_response_status(
        &self,
        status: StatusCode,
        _response: &reqwest::Response,
    ) -> Result<()> {
        if status.is_success() {
            return Ok(());
        }

        match status {
            StatusCode::UNAUTHORIZED => Err(Error::Auth(AuthError::TokenExpired)),
            StatusCode::TOO_MANY_REQUESTS => {
                // Rate limit error - 429
                Err(Error::api(429, "Rate limit exceeded", None))
            }
            StatusCode::FORBIDDEN | StatusCode::NOT_FOUND => Err(Error::api(
                status.as_u16(),
                format!("API returned {}", status),
                None,
            )),
            _ => Err(Error::api(
                status.as_u16(),
                format!("API error: {}", status),
                None,
            )),
        }
    }

    /// Wrap a request in Cloud Code format.
    fn wrap_request(
        &self,
        project_id: &str,
        model: &str,
        mut request: GoogleRequest,
    ) -> CloudCodeWrapper {
        // Add session ID for caching
        let session_id = derive_session_id(&request);
        request.session_id = Some(session_id);

        // Add system instruction with Antigravity identity
        let mut system_parts = vec![Part::text(ANTIGRAVITY_SYSTEM_INSTRUCTION)];

        // Append existing system instruction
        if let Some(sys) = &request.system_instruction {
            for part in &sys.parts {
                if let Some(text) = &part.text {
                    system_parts.push(Part::text(text));
                }
            }
        }

        request.system_instruction = Some(Content {
            role: Some("user".to_string()),
            parts: system_parts,
        });

        // Create wrapper
        let mut wrapper = CloudCodeWrapper::new(project_id, model, request);
        wrapper.request_id = Some(generate_request_id());

        wrapper
    }

    /// Get the signature cache.
    pub fn signature_cache(&self) -> &SignatureCache {
        &self.signature_cache
    }

    /// Clear cached project information.
    ///
    /// Forces re-discovery on the next request.
    pub async fn clear_project_cache(&self) {
        let mut project = self.project_id.write().await;
        *project = None;
        let mut managed = self.managed_project_id.write().await;
        *managed = None;
    }
}

/// Derive a stable session ID from the request.
fn derive_session_id(request: &GoogleRequest) -> String {
    // Find first user content
    let first_user_content = request
        .contents
        .iter()
        .find(|c| c.role.as_deref() == Some("user"))
        .map(|c| {
            c.parts
                .iter()
                .filter_map(|p| p.text.as_deref())
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default();

    // Hash to create stable ID
    let mut hasher = Sha256::new();
    hasher.update(first_user_content.as_bytes());
    let result = hasher.finalize();
    format!("{:x}", result)
}

/// Builder for [`CloudCodeClient`].
pub struct CloudCodeClientBuilder {
    token_provider: Option<Arc<dyn TokenProvider>>,
    http_builder: crate::providers::gemini::transport::http::HttpClientBuilder,
    signature_cache: Option<Arc<SignatureCache>>,
}

impl CloudCodeClientBuilder {
    /// Create a new builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the token provider.
    pub fn with_token_provider(mut self, provider: Arc<dyn TokenProvider>) -> Self {
        self.token_provider = Some(provider);
        self
    }

    /// Set the connect timeout.
    pub fn with_connect_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.http_builder = self.http_builder.connect_timeout(timeout);
        self
    }

    /// Set the request timeout.
    pub fn with_request_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.http_builder = self.http_builder.request_timeout(timeout);
        self
    }

    /// Set a custom base URL (for testing).
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.http_builder = self.http_builder.base_url(url);
        self
    }

    /// Set a custom signature cache.
    pub fn with_signature_cache(mut self, cache: Arc<SignatureCache>) -> Self {
        self.signature_cache = Some(cache);
        self
    }

    /// Build the client.
    ///
    /// # Panics
    ///
    /// Panics if no token provider was provided.
    pub fn build(self) -> CloudCodeClient {
        let token_provider = self.token_provider.expect("token provider is required");

        CloudCodeClient {
            token_provider,
            http: self.http_builder.build(),
            project_id: Arc::new(RwLock::new(None)),
            managed_project_id: Arc::new(RwLock::new(None)),
            signature_cache: self
                .signature_cache
                .unwrap_or_else(|| Arc::new(SignatureCache::default())),
        }
    }
}

impl Default for CloudCodeClientBuilder {
    fn default() -> Self {
        Self {
            token_provider: None,
            http_builder: crate::providers::gemini::transport::http::HttpClientBuilder::default(),
            signature_cache: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockTokenProvider;

    #[async_trait::async_trait]
    impl TokenProvider for MockTokenProvider {
        async fn get_token(&self, _provider: &str) -> std::result::Result<String, crate::auth::error::AuthError> {
            Ok("mock-token".to_string())
        }
    }



    #[test]
    fn test_client_builder() {
        use std::time::Duration;

        let _client = CloudCodeClient::builder()
            .with_token_provider(Arc::new(MockTokenProvider))
            .with_connect_timeout(Duration::from_secs(60))
            .with_request_timeout(Duration::from_secs(600))
            .build();

        // Just verify it builds without panicking
    }

    #[test]
    fn test_client_with_custom_signature_cache() {
        let cache = Arc::new(SignatureCache::default());
        let cache_clone = cache.clone();

        let client = CloudCodeClient::builder()
            .with_token_provider(Arc::new(MockTokenProvider))
            .with_signature_cache(cache)
            .build();

        // Verify same cache is used
        assert!(Arc::ptr_eq(&client.signature_cache, &cache_clone));
    }
}
