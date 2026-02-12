//! Cloud Code API client.
//!
//! This module provides the main client for interacting with the Cloud Code API.
//! It handles authentication, request/response conversion, and streaming.
//!
//! ## Example
//!
//! ```rust,ignore
//! use gaud::gemini::{CloudCodeClient, FileTokenStorage, MessagesRequest};
//!
//! # async fn example() -> gaud::gemini::Result<()> {
//! // Create client with file-based storage
//! let storage = FileTokenStorage::default_path()?;
//! let client = CloudCodeClient::builder()
//!     .with_storage(storage)
//!     .build()?;
//!
//! // Simple request
//! let response = client.messages()
//!     .model("claude-sonnet-4-5-thinking")
//!     .max_tokens(1024)
//!     .user_message("Hello, Claude!")
//!     .send()
//!     .await?;
//!
//! println!("{}", response.text());
//! # Ok(())
//! # }
//! ```

use std::pin::Pin;
use std::sync::Arc;

use futures::{Stream, StreamExt};
use reqwest::StatusCode;
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;
use tracing::{debug, info, instrument};

use crate::auth::gemini::{discover_project, OAuthFlow};
use crate::gemini::constants::{
    is_thinking_model, ANTIGRAVITY_SYSTEM_INSTRUCTION, API_PATH_GENERATE_CONTENT,
    API_PATH_STREAM_GENERATE_CONTENT,
};
use crate::gemini::convert::{convert_request, convert_response, SignatureCache};
use crate::gemini::error::{AuthError, Error, Result};
use crate::gemini::models::content::ContentBlock;
use crate::gemini::models::google::{CloudCodeWrapper, Content, GoogleRequest, Part};
use crate::gemini::models::request::{Message, MessageContent, MessagesRequest, Role, SystemPrompt};
use crate::gemini::models::response::{MessagesResponse, StopReason, Usage};
use crate::gemini::models::stream::StreamEvent;
use crate::gemini::models::tools::{Tool, ToolChoice};
use crate::gemini::storage::TokenStorage;
use crate::gemini::transport::http::{generate_request_id, HttpClient};
use crate::gemini::transport::sse::SseStream;

/// Cloud Code API client.
///
/// Generic over the token storage backend.
///
/// # Thread Safety
///
/// The client is fully thread-safe. All internal state is protected by
/// `Arc<RwLock<_>>` and can be safely shared across tasks.
#[derive(Clone)]
pub struct CloudCodeClient<S: TokenStorage> {
    /// OAuth flow for token management.
    oauth: Arc<OAuthFlow<S>>,
    /// HTTP client for API requests.
    http: HttpClient,
    /// Cached project ID.
    project_id: Arc<RwLock<Option<String>>>,
    /// Cached managed project ID.
    managed_project_id: Arc<RwLock<Option<String>>>,
    /// Signature cache for thinking signatures.
    signature_cache: Arc<SignatureCache>,
}

impl<S: TokenStorage + 'static> CloudCodeClient<S> {
    /// Create a new client builder.
    pub fn builder() -> CloudCodeClientBuilder<S> {
        CloudCodeClientBuilder::default()
    }

    /// Create a new client with the given storage.
    ///
    /// Prefer using the builder for more configuration options.
    pub fn new(storage: S) -> Self {
        Self::builder().with_storage(storage).build()
    }

    /// Start a new messages request builder.
    ///
    /// This provides a fluent API for constructing requests.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let client = Arc::new(CloudCodeClient::new(storage));
    /// let response = client.messages()
    ///     .model("claude-sonnet-4-5")
    ///     .max_tokens(1024)
    ///     .system("You are a helpful assistant.")
    ///     .user_message("Hello!")
    ///     .send()
    ///     .await?;
    /// ```
    pub fn messages(self: Arc<Self>) -> MessagesRequestBuilder<S> {
        MessagesRequestBuilder::new(self)
    }

    /// Check if the client is authenticated.
    ///
    /// Returns `true` if a valid (non-expired) token exists.
    pub async fn is_authenticated(&self) -> Result<bool> {
        self.oauth.is_authenticated().await
    }

    /// Start the OAuth authorization flow.
    ///
    /// Returns the authorization URL and OAuth state. The user should open
    /// the URL in a browser to authenticate.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let (url, state) = client.start_oauth_flow().await?;
    /// println!("Open this URL to authenticate: {}", url);
    /// // User completes authentication...
    /// client.complete_oauth_flow(code, &state.state).await?;
    /// ```
    pub async fn start_oauth_flow(&self) -> Result<(String, crate::auth::gemini::OAuthFlowState)> {
        self.oauth.start_authorization_async().await
    }

    /// Complete the OAuth authorization flow.
    ///
    /// Call this with the authorization code after the user completes authentication.
    ///
    /// # Arguments
    ///
    /// * `code` - The authorization code from the OAuth callback
    /// * `state` - The state parameter for CSRF protection (optional but recommended)
    pub async fn complete_oauth_flow(
        &self,
        code: &str,
        state: Option<&str>,
    ) -> Result<crate::auth::gemini::TokenInfo> {
        self.oauth.exchange_code(code, state).await
    }

    /// Get the current access token.
    ///
    /// Automatically refreshes the token if it's expired.
    pub async fn get_access_token(&self) -> Result<String> {
        self.oauth.get_access_token().await
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

    /// Send a messages request (non-streaming).
    ///
    /// # Arguments
    ///
    /// * `request` - The messages request to send
    ///
    /// # Returns
    ///
    /// The complete response after the model finishes generating.
    #[instrument(skip(self, request), fields(model = %request.model))]
    pub async fn request(&self, request: &MessagesRequest) -> Result<MessagesResponse> {
        let token = self.get_access_token().await?;
        let project_id = self.get_project_id().await?;

        debug!(
            model = %request.model,
            max_tokens = request.max_tokens,
            messages = request.messages.len(),
            "Sending request"
        );

        // Convert to Google format
        let google_request = convert_request(request);

        // Wrap in Cloud Code format
        let wrapped = self.wrap_request(&project_id, &request.model, google_request, request);

        // Send request
        let path = API_PATH_GENERATE_CONTENT;
        let is_thinking = is_thinking_model(&request.model);

        // For thinking models, use streaming endpoint even for non-streaming
        // to properly accumulate thinking blocks
        if is_thinking {
            return self
                .request_via_streaming(request, &token, &project_id, &wrapped)
                .await;
        }

        let response = self
            .http
            .post_with_fallback(path, &token, &wrapped, &request.model, false)
            .await?;

        let status = response.status();
        self.handle_response_status(status, &response).await?;

        // Parse response
        let google_response: crate::gemini::models::google::GoogleResponse = response.json().await?;
        let anthropic_response = convert_response(&google_response, &request.model);

        debug!(
            stop_reason = ?anthropic_response.stop_reason,
            input_tokens = anthropic_response.usage.input_tokens,
            output_tokens = anthropic_response.usage.output_tokens,
            "Request completed"
        );

        Ok(anthropic_response)
    }

    /// Send a messages request and return a stream of events.
    ///
    /// # Arguments
    ///
    /// * `request` - The messages request to send
    ///
    /// # Returns
    ///
    /// A stream of `StreamEvent`s.
    #[instrument(skip(self, request), fields(model = %request.model))]
    pub async fn request_stream(
        &self,
        request: &MessagesRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        let token = self.get_access_token().await?;
        let project_id = self.get_project_id().await?;

        debug!(
            model = %request.model,
            max_tokens = request.max_tokens,
            messages = request.messages.len(),
            "Starting streaming request"
        );

        // Convert to Google format
        let google_request = convert_request(request);

        // Wrap in Cloud Code format
        let wrapped = self.wrap_request(&project_id, &request.model, google_request, request);

        // Send streaming request
        let path = API_PATH_STREAM_GENERATE_CONTENT;
        let response = self
            .http
            .post_with_fallback(path, &token, &wrapped, &request.model, true)
            .await?;

        let status = response.status();
        self.handle_response_status(status, &response).await?;

        // Create SSE stream
        let byte_stream = response.bytes_stream();
        let sse_stream = SseStream::new(byte_stream, &request.model);

        Ok(Box::pin(sse_stream))
    }

    /// Send request via streaming endpoint and accumulate response.
    ///
    /// Used for thinking models where we need to use streaming to get
    /// thinking blocks properly.
    async fn request_via_streaming(
        &self,
        request: &MessagesRequest,
        token: &str,
        _project_id: &str,
        wrapped: &CloudCodeWrapper,
    ) -> Result<MessagesResponse> {
        let path = API_PATH_STREAM_GENERATE_CONTENT;
        let response = self
            .http
            .post_with_fallback(path, token, wrapped, &request.model, true)
            .await?;

        let status = response.status();
        self.handle_response_status(status, &response).await?;

        // Accumulate stream events into a response
        let byte_stream = response.bytes_stream();
        let mut sse_stream = SseStream::new(byte_stream, &request.model);

        let mut content: Vec<ContentBlock> = Vec::new();
        let mut current_text = String::new();
        let mut current_thinking = String::new();
        let mut current_signature: Option<String> = None;
        let mut usage = Usage::default();
        let mut stop_reason: Option<StopReason> = None;
        let mut message_id = String::new();

        while let Some(event) = sse_stream.next().await {
            let event = event?;

            match event {
                StreamEvent::MessageStart { message } => {
                    message_id = message.id;
                    usage = message.usage.clone().unwrap_or_default();
                }
                StreamEvent::ContentBlockStart { content_block, .. } => {
                    // Flush any accumulated content
                    if !current_text.is_empty() {
                        content.push(ContentBlock::text(&current_text));
                        current_text.clear();
                    }
                    if !current_thinking.is_empty() {
                        content.push(ContentBlock::thinking(
                            &current_thinking,
                            current_signature.take(),
                        ));
                        current_thinking.clear();
                    }

                    // Handle tool_use start
                    if content_block.is_tool_use() {
                        if let Some((id, name, input)) = content_block.as_tool_use() {
                            content.push(ContentBlock::tool_use(id, name, input.clone()));
                        }
                    }
                }
                StreamEvent::ContentBlockDelta { delta, .. } => {
                    match delta {
                        crate::gemini::models::stream::ContentDelta::TextDelta { text } => {
                            current_text.push_str(&text);
                        }
                        crate::gemini::models::stream::ContentDelta::ThinkingDelta { thinking } => {
                            current_thinking.push_str(&thinking);
                        }
                        crate::gemini::models::stream::ContentDelta::SignatureDelta { signature } => {
                            current_signature = Some(signature);
                        }
                        crate::gemini::models::stream::ContentDelta::InputJsonDelta { partial_json: _ } => {
                            // Tool input is handled by content_block_start
                        }
                    }
                }
                StreamEvent::ContentBlockStop { .. } => {
                    // Flush accumulated content
                    if !current_text.is_empty() {
                        content.push(ContentBlock::text(&current_text));
                        current_text.clear();
                    }
                    if !current_thinking.is_empty() {
                        content.push(ContentBlock::thinking(
                            &current_thinking,
                            current_signature.take(),
                        ));
                        current_thinking.clear();
                    }
                }
                StreamEvent::MessageDelta {
                    delta,
                    usage: delta_usage,
                } => {
                    stop_reason = delta.stop_reason;
                    if let Some(u) = delta_usage {
                        usage.output_tokens = u.output_tokens;
                        if let Some(cache_read) = u.cache_read_input_tokens {
                            usage.cache_read_input_tokens = Some(cache_read);
                        }
                    }
                }
                StreamEvent::MessageStop => {
                    break;
                }
                StreamEvent::Error { error } => {
                    return Err(Error::api(500, error.message, None));
                }
                _ => {}
            }
        }

        // Flush any remaining content
        if !current_text.is_empty() {
            content.push(ContentBlock::text(&current_text));
        }
        if !current_thinking.is_empty() {
            content.push(ContentBlock::thinking(&current_thinking, current_signature));
        }

        // Ensure at least one content block
        if content.is_empty() {
            content.push(ContentBlock::text(""));
        }

        Ok(MessagesResponse {
            id: message_id,
            response_type: "message".to_string(),
            model: request.model.clone(),
            role: Role::Assistant,
            content,
            stop_reason,
            stop_sequence: None,
            usage,
        })
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
        original_request: &MessagesRequest,
    ) -> CloudCodeWrapper {
        // Add session ID for caching (future use for cache continuity)
        let session_id = derive_session_id(original_request);
        request.session_id = Some(session_id);

        // Add system instruction with Antigravity identity
        let mut system_parts = vec![
            Part::text(ANTIGRAVITY_SYSTEM_INSTRUCTION),
            Part::text(format!(
                "Please ignore the following [ignore]{}[/ignore]",
                ANTIGRAVITY_SYSTEM_INSTRUCTION
            )),
        ];

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
///
/// The session ID is derived from the first user message content,
/// providing cache continuity across turns in a conversation.
fn derive_session_id(request: &MessagesRequest) -> String {
    // Find first user message
    let first_user_content = request
        .messages
        .iter()
        .find(|m| m.role == Role::User)
        .map(|m| match &m.content {
            MessageContent::Text(text) => text.clone(),
            MessageContent::Blocks(blocks) => blocks
                .iter()
                .filter_map(|b| b.as_text())
                .collect::<Vec<_>>()
                .join(""),
        })
        .unwrap_or_default();

    // Hash to create stable ID
    let mut hasher = Sha256::new();
    hasher.update(first_user_content.as_bytes());
    let result = hasher.finalize();
    format!("{:x}", result)
}

/// Builder for [`CloudCodeClient`].
///
/// # Example
///
/// ```rust,ignore
/// use gaud::gemini::{CloudCodeClient, FileTokenStorage};
/// use std::time::Duration;
///
/// let client = CloudCodeClient::builder()
///     .with_storage(FileTokenStorage::default_path()?)
///     .with_connect_timeout(Duration::from_secs(30))
///     .with_request_timeout(Duration::from_secs(300))
///     .build();
/// ```
pub struct CloudCodeClientBuilder<S: TokenStorage> {
    storage: Option<S>,
    http_builder: crate::gemini::transport::http::HttpClientBuilder,
    oauth_config: Option<crate::gemini::constants::OAuthConfig>,
    signature_cache: Option<Arc<SignatureCache>>,
}

impl<S: TokenStorage + 'static> CloudCodeClientBuilder<S> {
    /// Create a new builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the token storage backend.
    pub fn with_storage(mut self, storage: S) -> Self {
        self.storage = Some(storage);
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

    /// Set a custom OAuth configuration.
    pub fn with_oauth_config(mut self, config: crate::gemini::constants::OAuthConfig) -> Self {
        self.oauth_config = Some(config);
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
    /// Panics if no storage backend was provided.
    pub fn build(self) -> CloudCodeClient<S> {
        let storage = self.storage.expect("storage is required");
        let oauth = OAuthFlow::new(storage);

        CloudCodeClient {
            oauth: Arc::new(oauth),
            http: self.http_builder.build(),
            project_id: Arc::new(RwLock::new(None)),
            managed_project_id: Arc::new(RwLock::new(None)),
            signature_cache: self
                .signature_cache
                .unwrap_or_else(|| Arc::new(SignatureCache::default())),
        }
    }
}

impl<S: TokenStorage> Default for CloudCodeClientBuilder<S> {
    fn default() -> Self {
        Self {
            storage: None,
            http_builder: crate::gemini::transport::http::HttpClientBuilder::default(),
            oauth_config: None,
            signature_cache: None,
        }
    }
}

/// Builder for constructing messages requests with a fluent API.
///
/// # Example
///
/// ```rust,ignore
/// let client = Arc::new(CloudCodeClient::new(storage));
/// let response = client.messages()
///     .model("claude-sonnet-4-5-thinking")
///     .max_tokens(2048)
///     .system("You are a helpful coding assistant.")
///     .user_message("Write a function to sort a list.")
///     .thinking_budget(10000)
///     .send()
///     .await?;
/// ```
pub struct MessagesRequestBuilder<S: TokenStorage> {
    client: Arc<CloudCodeClient<S>>,
    model: Option<String>,
    max_tokens: Option<u32>,
    messages: Vec<Message>,
    system: Option<SystemPrompt>,
    temperature: Option<f32>,
    top_p: Option<f32>,
    top_k: Option<u32>,
    stop_sequences: Option<Vec<String>>,
    tools: Option<Vec<Tool>>,
    tool_choice: Option<ToolChoice>,
    thinking_budget: Option<u32>,
}

impl<S: TokenStorage + 'static> MessagesRequestBuilder<S> {
    /// Create a new builder with the given client.
    fn new(client: Arc<CloudCodeClient<S>>) -> Self {
        Self {
            client,
            model: None,
            max_tokens: None,
            messages: Vec::new(),
            system: None,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: None,
            tools: None,
            tool_choice: None,
            thinking_budget: None,
        }
    }

    /// Set the model to use.
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Set the maximum tokens to generate.
    pub fn max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = Some(max_tokens);
        self
    }

    /// Add a message to the conversation.
    pub fn message(mut self, message: Message) -> Self {
        self.messages.push(message);
        self
    }

    /// Add a user message with text content.
    pub fn user_message(mut self, content: impl Into<String>) -> Self {
        self.messages.push(Message::user(content));
        self
    }

    /// Add an assistant message with text content.
    pub fn assistant_message(mut self, content: impl Into<String>) -> Self {
        self.messages.push(Message::assistant(content));
        self
    }

    /// Set the system prompt as a string.
    pub fn system(mut self, system: impl Into<String>) -> Self {
        self.system = Some(SystemPrompt::Text(system.into()));
        self
    }

    /// Set the system prompt as blocks.
    pub fn system_blocks(mut self, blocks: Vec<crate::gemini::models::request::SystemBlock>) -> Self {
        self.system = Some(SystemPrompt::Blocks(blocks));
        self
    }

    /// Set the sampling temperature (0.0 to 1.0).
    pub fn temperature(mut self, temp: f32) -> Self {
        self.temperature = Some(temp);
        self
    }

    /// Set the top-p sampling parameter.
    pub fn top_p(mut self, top_p: f32) -> Self {
        self.top_p = Some(top_p);
        self
    }

    /// Set the top-k sampling parameter.
    pub fn top_k(mut self, top_k: u32) -> Self {
        self.top_k = Some(top_k);
        self
    }

    /// Set stop sequences.
    pub fn stop_sequences(mut self, sequences: Vec<String>) -> Self {
        self.stop_sequences = Some(sequences);
        self
    }

    /// Add a tool definition.
    pub fn tool(mut self, tool: Tool) -> Self {
        self.tools.get_or_insert_with(Vec::new).push(tool);
        self
    }

    /// Set all tools at once.
    pub fn tools(mut self, tools: Vec<Tool>) -> Self {
        self.tools = Some(tools);
        self
    }

    /// Set the tool choice strategy.
    pub fn tool_choice(mut self, choice: ToolChoice) -> Self {
        self.tool_choice = Some(choice);
        self
    }

    /// Set the thinking budget for thinking models.
    pub fn thinking_budget(mut self, budget: u32) -> Self {
        self.thinking_budget = Some(budget);
        self
    }

    /// Enable thinking with the given budget.
    ///
    /// Convenience method that sets the thinking budget.
    pub fn enable_thinking(self, budget: u32) -> Self {
        self.thinking_budget(budget)
    }

    /// Build the request without sending.
    pub fn build_request(&self) -> Result<MessagesRequest> {
        let model = self
            .model
            .clone()
            .ok_or_else(|| Error::config("model is required"))?;
        let max_tokens = self
            .max_tokens
            .ok_or_else(|| Error::config("max_tokens is required"))?;

        if self.messages.is_empty() {
            return Err(Error::config("at least one message is required"));
        }

        let mut builder = MessagesRequest::builder()
            .model(&model)
            .max_tokens(max_tokens);

        for msg in &self.messages {
            builder = builder.message(msg.clone());
        }

        if let Some(system) = &self.system {
            match system {
                SystemPrompt::Text(text) => {
                    builder = builder.system(text);
                }
                SystemPrompt::Blocks(blocks) => {
                    builder = builder.system_blocks(blocks.clone());
                }
            }
        }

        if let Some(temp) = self.temperature {
            builder = builder.temperature(temp);
        }

        if let Some(top_p) = self.top_p {
            builder = builder.top_p(top_p);
        }

        if let Some(top_k) = self.top_k {
            builder = builder.top_k(top_k);
        }

        if let Some(sequences) = &self.stop_sequences {
            builder = builder.stop_sequences(sequences.clone());
        }

        if let Some(tools) = &self.tools {
            for tool in tools {
                builder = builder.tool(tool.clone());
            }
        }

        if let Some(choice) = &self.tool_choice {
            builder = builder.tool_choice(choice.clone());
        }

        if let Some(budget) = self.thinking_budget {
            builder = builder.thinking(budget);
        }

        Ok(builder.build())
    }

    /// Send the request (non-streaming).
    pub async fn send(self) -> Result<MessagesResponse> {
        let request = self.build_request()?;
        self.client.request(&request).await
    }

    /// Send the request as a stream.
    pub async fn send_stream(self) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        let request = self.build_request()?;
        self.client.request_stream(&request).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gemini::storage::MemoryTokenStorage;

    fn create_test_client() -> Arc<CloudCodeClient<MemoryTokenStorage>> {
        Arc::new(CloudCodeClient::new(MemoryTokenStorage::new()))
    }

    #[test]
    fn test_derive_session_id() {
        let request1 = MessagesRequest::simple("claude-sonnet-4-5", 1024, "Hello!");
        let request2 = MessagesRequest::simple("claude-sonnet-4-5", 1024, "Hello!");
        let request3 = MessagesRequest::simple("claude-sonnet-4-5", 1024, "Goodbye!");

        let id1 = derive_session_id(&request1);
        let id2 = derive_session_id(&request2);
        let id3 = derive_session_id(&request3);

        // Same content should produce same ID
        assert_eq!(id1, id2);
        // Different content should produce different ID
        assert_ne!(id1, id3);
    }

    #[test]
    fn test_messages_request_builder_validation() {
        let client = create_test_client();

        // Missing model
        let result = Arc::clone(&client)
            .messages()
            .max_tokens(1024)
            .user_message("Hello")
            .build_request();
        assert!(result.is_err());

        // Missing max_tokens
        let result = Arc::clone(&client)
            .messages()
            .model("claude-sonnet-4-5")
            .user_message("Hello")
            .build_request();
        assert!(result.is_err());

        // Missing messages
        let result = Arc::clone(&client)
            .messages()
            .model("claude-sonnet-4-5")
            .max_tokens(1024)
            .build_request();
        assert!(result.is_err());

        // Valid request
        let result = Arc::clone(&client)
            .messages()
            .model("claude-sonnet-4-5")
            .max_tokens(1024)
            .user_message("Hello")
            .build_request();
        assert!(result.is_ok());
    }

    #[test]
    fn test_messages_request_builder_full() {
        let client = create_test_client();

        let tool = Tool::new(
            "get_weather",
            "Get weather for a location",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "location": { "type": "string" }
                }
            }),
        );

        let request = client
            .messages()
            .model("claude-sonnet-4-5-thinking")
            .max_tokens(2048)
            .system("You are helpful.")
            .user_message("Hello!")
            .assistant_message("Hi there!")
            .user_message("What's the weather?")
            .temperature(0.7)
            .top_p(0.9)
            .top_k(40)
            .stop_sequences(vec!["END".to_string()])
            .tool(tool)
            .tool_choice(ToolChoice::Auto)
            .thinking_budget(10000)
            .build_request()
            .unwrap();

        assert_eq!(request.model, "claude-sonnet-4-5-thinking");
        assert_eq!(request.max_tokens, 2048);
        assert_eq!(request.messages.len(), 3);
        assert!(request.system.is_some());
        assert_eq!(request.temperature, Some(0.7));
        assert_eq!(request.top_p, Some(0.9));
        assert_eq!(request.top_k, Some(40));
        assert!(request.tools.is_some());
        assert!(request.thinking.is_some());
    }

    #[test]
    fn test_client_builder() {
        use std::time::Duration;

        let client = CloudCodeClient::builder()
            .with_storage(MemoryTokenStorage::new())
            .with_connect_timeout(Duration::from_secs(60))
            .with_request_timeout(Duration::from_secs(600))
            .build();

        // Just verify it builds without panicking
        assert!(client.project_id.try_read().is_ok());
    }

    #[test]
    fn test_client_with_custom_signature_cache() {
        let cache = Arc::new(SignatureCache::default());
        let cache_clone = cache.clone();

        let client = CloudCodeClient::builder()
            .with_storage(MemoryTokenStorage::new())
            .with_signature_cache(cache)
            .build();

        // Verify same cache is used
        assert!(Arc::ptr_eq(&client.signature_cache, &cache_clone));
    }
}
