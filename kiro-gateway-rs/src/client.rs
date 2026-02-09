//! Main client entry point.

use std::pin::Pin;
use std::sync::Arc;

use async_stream::try_stream;
use futures::{Stream, StreamExt};
use tracing::{debug, info};

use crate::api::messages::MessagesRequestBuilder;
use crate::auth::KiroAuthManager;
use crate::convert::model_resolver::ModelResolver;
use crate::convert::request::build_kiro_payload;
use crate::convert::response::ResponseAccumulator;
use crate::error::{Error, Result};
use crate::models::request::MessagesRequest;
use crate::models::response::MessagesResponse;
use crate::models::stream::StreamEvent;
use crate::transport::http::KiroHttpClient;
use crate::transport::sse;

/// Kiro API client.
///
/// Provides both an Anthropic Messages API surface and raw Kiro API access.
///
/// # Examples
///
/// ```rust,no_run
/// use kiro_gateway::{KiroClient, KiroClientBuilder};
///
/// # async fn example() -> kiro_gateway::Result<()> {
/// let client = KiroClientBuilder::new()
///     .credentials_file("~/.kiro/credentials.json")
///     .build()
///     .await?;
///
/// let response = client.messages()
///     .model("claude-sonnet-4.5")
///     .max_tokens(1024)
///     .user_message("Hello, Claude!")
///     .send()
///     .await?;
///
/// println!("{}", response.text());
/// # Ok(())
/// # }
/// ```
pub struct KiroClient {
    auth: Arc<KiroAuthManager>,
    http: Arc<KiroHttpClient>,
    model_resolver: Arc<ModelResolver>,
}

impl KiroClient {
    /// Create a builder for configuring the client.
    pub fn builder() -> KiroClientBuilder {
        KiroClientBuilder::new()
    }

    /// Start building a Messages API request.
    pub fn messages(&self) -> MessagesRequestBuilder<'_> {
        MessagesRequestBuilder::new(self)
    }

    /// Send a Messages API request and get a complete response.
    pub async fn send_messages(&self, request: MessagesRequest) -> Result<MessagesResponse> {
        let model_id = self.model_resolver.resolve(&request.model);
        let region = self.auth.region().await;
        let profile_arn = self.auth.profile_arn().await;

        let payload = build_kiro_payload(&request, &model_id, profile_arn.as_deref())?;
        let url =
            crate::config::generate_assistant_response_url(&region, profile_arn.as_deref());

        debug!(model = model_id.as_str(), "Sending Messages request");

        let response = self.http.post_streaming(&url, &payload).await?;
        let body = response.text().await.map_err(|e| {
            Error::Stream(format!("Failed to read response body: {}", e))
        })?;

        // Parse the streaming response into a complete response
        let mut accumulator = ResponseAccumulator::new(&model_id);
        let events = sse::parse_chunk(&body);
        for event in events {
            accumulator.process_event(event);
        }

        Ok(accumulator.into_response())
    }

    /// Send a Messages API request and get a streaming response.
    pub async fn send_messages_stream(
        &self,
        request: MessagesRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        let model_id = self.model_resolver.resolve(&request.model);
        let region = self.auth.region().await;
        let profile_arn = self.auth.profile_arn().await;

        let payload = build_kiro_payload(&request, &model_id, profile_arn.as_deref())?;
        let url =
            crate::config::generate_assistant_response_url(&region, profile_arn.as_deref());

        debug!(model = model_id.as_str(), "Sending streaming Messages request");

        let response = self.http.post_streaming(&url, &payload).await?;
        let model_id_owned = model_id.clone();

        let stream = try_stream! {
            let mut accumulator = ResponseAccumulator::new(&model_id_owned);

            // Emit message_start
            yield accumulator.message_start_event();
            yield accumulator.text_block_start_event();

            let mut byte_stream = response.bytes_stream();
            let mut buffer = String::new();

            while let Some(chunk_result) = byte_stream.next().await {
                let chunk = chunk_result.map_err(|e| Error::Stream(format!("Stream read error: {}", e)))?;
                let chunk_str = String::from_utf8_lossy(&chunk);
                buffer.push_str(&chunk_str);

                // Process complete lines from the buffer
                while let Some(newline_pos) = buffer.find('\n') {
                    let line = buffer[..newline_pos].trim().to_string();
                    buffer = buffer[newline_pos + 1..].to_string();

                    if line.is_empty() {
                        continue;
                    }

                    let events = sse::parse_chunk(&line);
                    for event in events {
                        let stream_events = accumulator.process_event(event);
                        for se in stream_events {
                            yield se;
                        }
                    }
                }
            }

            // Process any remaining buffered data
            if !buffer.trim().is_empty() {
                let events = sse::parse_chunk(&buffer);
                for event in events {
                    let stream_events = accumulator.process_event(event);
                    for se in stream_events {
                        yield se;
                    }
                }
            }

            // Emit finish events
            for event in accumulator.finish_events() {
                yield event;
            }
        };

        Ok(Box::pin(stream))
    }

    /// List available models.
    pub async fn list_models(&self) -> Result<Vec<String>> {
        let region = self.auth.region().await;
        let profile_arn = self.auth.profile_arn().await;
        crate::api::models::list_models(&self.http, &region, profile_arn.as_deref()).await
    }

    /// Send a raw Kiro API payload.
    pub async fn raw_request(&self, payload: &serde_json::Value) -> Result<String> {
        let region = self.auth.region().await;
        let profile_arn = self.auth.profile_arn().await;
        crate::api::raw::raw_request(&self.http, &region, profile_arn.as_deref(), payload).await
    }

    /// Send a raw Kiro API payload and get a streaming response.
    pub async fn raw_request_stream(
        &self,
        payload: &serde_json::Value,
    ) -> Result<reqwest::Response> {
        let region = self.auth.region().await;
        let profile_arn = self.auth.profile_arn().await;
        crate::api::raw::raw_request_stream(&self.http, &region, profile_arn.as_deref(), payload)
            .await
    }

    /// Get a reference to the auth manager.
    pub fn auth(&self) -> &KiroAuthManager {
        &self.auth
    }

    /// Get a reference to the model resolver.
    pub fn model_resolver(&self) -> &ModelResolver {
        &self.model_resolver
    }
}

/// Builder for [`KiroClient`].
pub struct KiroClientBuilder {
    credentials_file: Option<String>,
    sqlite_db: Option<String>,
    refresh_token: Option<String>,
    region: Option<String>,
    profile_arn: Option<String>,
    storage: Option<Arc<dyn crate::storage::TokenStorage>>,
    reqwest_client: Option<reqwest::Client>,
}

impl KiroClientBuilder {
    /// Create a new builder.
    pub fn new() -> Self {
        Self {
            credentials_file: None,
            sqlite_db: None,
            refresh_token: None,
            region: None,
            profile_arn: None,
            storage: None,
            reqwest_client: None,
        }
    }

    /// Load credentials from a JSON file.
    pub fn credentials_file(mut self, path: impl Into<String>) -> Self {
        self.credentials_file = Some(path.into());
        self
    }

    /// Load credentials from a SQLite database.
    pub fn sqlite_db(mut self, path: impl Into<String>) -> Self {
        self.sqlite_db = Some(path.into());
        self
    }

    /// Set a refresh token directly.
    pub fn refresh_token(mut self, token: impl Into<String>) -> Self {
        self.refresh_token = Some(token.into());
        self
    }

    /// Set the AWS region.
    pub fn region(mut self, region: impl Into<String>) -> Self {
        self.region = Some(region.into());
        self
    }

    /// Set the profile ARN.
    pub fn profile_arn(mut self, arn: impl Into<String>) -> Self {
        self.profile_arn = Some(arn.into());
        self
    }

    /// Set a token storage backend.
    pub fn storage(mut self, storage: Arc<dyn crate::storage::TokenStorage>) -> Self {
        self.storage = Some(storage);
        self
    }

    /// Set a custom reqwest client.
    pub fn reqwest_client(mut self, client: reqwest::Client) -> Self {
        self.reqwest_client = Some(client);
        self
    }

    /// Build the client, loading and validating credentials.
    pub async fn build(self) -> Result<KiroClient> {
        let token_info = self.load_credentials()?;

        let mut auth_manager = KiroAuthManager::new(token_info);
        if let Some(storage) = self.storage {
            auth_manager = auth_manager.with_storage(storage);
        }
        if let Some(client) = &self.reqwest_client {
            auth_manager = auth_manager.with_client(client.clone());
        }

        let auth = Arc::new(auth_manager);
        let http = Arc::new(KiroHttpClient::new(Arc::clone(&auth)));
        let model_resolver = Arc::new(ModelResolver::new());

        info!("KiroClient initialized");
        Ok(KiroClient {
            auth,
            http,
            model_resolver,
        })
    }

    fn load_credentials(&self) -> Result<crate::models::auth::KiroTokenInfo> {
        // Priority: SQLite > JSON file > env > direct refresh token

        // 1. SQLite database
        if let Some(db_path) = &self.sqlite_db {
            let mut token = crate::auth::credentials::load_from_sqlite(db_path)?;
            self.apply_overrides(&mut token);
            return Ok(token);
        }

        // 2. JSON credentials file
        if let Some(file_path) = &self.credentials_file {
            let mut token = crate::auth::credentials::load_from_json_file(file_path)?;
            self.apply_overrides(&mut token);
            return Ok(token);
        }

        // 3. Environment variables
        if let Some(mut token) = crate::auth::credentials::load_from_env() {
            self.apply_overrides(&mut token);
            return Ok(token);
        }

        // 4. Direct refresh token
        if let Some(refresh_token) = &self.refresh_token {
            let mut token = crate::models::auth::KiroTokenInfo::new(refresh_token.clone());
            self.apply_overrides(&mut token);
            token.detect_auth_type();
            return Ok(token);
        }

        // 5. Try default paths
        // Default SQLite path
        let default_sqlite = "~/.local/share/kiro-cli/data.sqlite3";
        if let Ok(mut token) = crate::auth::credentials::load_from_sqlite(default_sqlite) {
            self.apply_overrides(&mut token);
            return Ok(token);
        }

        Err(Error::NotAuthenticated)
    }

    fn apply_overrides(&self, token: &mut crate::models::auth::KiroTokenInfo) {
        if let Some(region) = &self.region {
            token.region = region.clone();
        }
        if let Some(arn) = &self.profile_arn {
            token.profile_arn = Some(arn.clone());
        }
    }
}

impl Default for KiroClientBuilder {
    fn default() -> Self {
        Self::new()
    }
}
