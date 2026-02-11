//! Kiro Provider (Amazon Q / AWS CodeWhisperer)
//!
//! Routes requests through the Kiro API using the Anthropic Messages API format
//! internally. This provider converts between the gaud OpenAI-compatible format
//! and the Anthropic Messages format that the Kiro service expects.
//!
//! Format conversion is delegated to [`KiroTransformer`] and [`KiroStreamState`]
//! (in `providers::transform::kiro`), which are well-tested independently.
//! This module handles transport (via `reqwest`) and error mapping.
//!
//! ## Auth flow
//!
//! The Kiro API requires an access token obtained via an OAuth-like
//! refresh-token flow from `prod.{region}.auth.desktop.kiro.dev/refreshToken`.
//! The [`KiroAuthManager`] handles refresh, token caching, and expiry.
//!
//! ## API endpoint
//!
//! Requests go to `https://q.{region}.amazonaws.com/generateAssistantResponse`
//! with a required `?origin=AI_EDITOR` query parameter.

use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use futures::stream::StreamExt;
use futures::Stream;
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::RwLock;
use tracing::{debug, info};
use uuid::Uuid;

use crate::providers::pricing::ModelPricing;
use crate::providers::transform::kiro::KiroTransformer;
use crate::providers::transformer::{ProviderResponseMeta, ProviderTransformer};
use crate::providers::types::*;
use crate::providers::{LlmProvider, ProviderError};

// ---------------------------------------------------------------------------
// Constants – match kiro-aws/kiro-gateway reference exactly
// ---------------------------------------------------------------------------

/// URL template for Kiro Desktop Auth token refresh.
const KIRO_REFRESH_URL_TEMPLATE: &str =
    "https://prod.{region}.auth.desktop.kiro.dev/refreshToken";

/// URL template for the Kiro API host.
/// Fixed in kiro-gateway issue #58 – `q.{region}` works for all regions.
const KIRO_API_HOST_TEMPLATE: &str = "https://q.{region}.amazonaws.com";

/// Origin query parameter sent with every Kiro API request.
const API_ORIGIN: &str = "AI_EDITOR";

/// Refresh token when it expires within this window (10 minutes).
const TOKEN_REFRESH_THRESHOLD: Duration = Duration::from_secs(600);

/// Safety margin subtracted from the token's reported expiry (60 s).
const EXPIRY_SAFETY_MARGIN: Duration = Duration::from_secs(60);

// ---------------------------------------------------------------------------
// URL helpers
// ---------------------------------------------------------------------------

fn kiro_refresh_url(region: &str) -> String {
    KIRO_REFRESH_URL_TEMPLATE.replace("{region}", region)
}

fn kiro_api_host(region: &str) -> String {
    KIRO_API_HOST_TEMPLATE.replace("{region}", region)
}

/// Percent-encode a string for use in URL query parameters (RFC 3986).
fn url_encode(s: &str) -> String {
    urlencoding::encode(s).into_owned()
}

fn generate_assistant_response_url(region: &str, profile_arn: Option<&str>) -> String {
    let host = kiro_api_host(region);
    match profile_arn {
        Some(arn) => format!(
            "{host}/generateAssistantResponse?origin={API_ORIGIN}&profileArn={}",
            url_encode(arn)
        ),
        None => format!("{host}/generateAssistantResponse?origin={API_ORIGIN}"),
    }
}

/// Generate a machine fingerprint for User-Agent (SHA-256 of hostname-user).
fn machine_fingerprint() -> String {
    use sha2::{Digest, Sha256};
    let hostname = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    let username = whoami::username_os()
        .map(|u| u.to_string_lossy().to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    let input = format!("{hostname}-{username}-kiro-gateway");
    let hash = Sha256::digest(input.as_bytes());
    // Inline hex encoding to avoid pulling in the `hex` crate
    hash.iter().map(|b| format!("{b:02x}")).collect::<String>()
}

/// Build the headers that the Kiro API expects. These mirror `get_kiro_headers`
/// in the Python reference.
fn kiro_headers(token: &str, fingerprint: &str) -> reqwest::header::HeaderMap {
    use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

    let mut headers = HeaderMap::new();
    headers.insert(
        "authorization",
        HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
    );
    headers.insert("content-type", HeaderValue::from_static("application/json"));

    let ua = format!(
        "aws-sdk-js/1.0.27 ua/2.1 os/linux lang/js md/nodejs#22.21.1 \
         api/codewhispererstreaming#1.0.27 m/E KiroIDE-0.7.45-{fingerprint}"
    );
    headers.insert("user-agent", HeaderValue::from_str(&ua).unwrap());
    headers.insert(
        "x-amz-user-agent",
        HeaderValue::from_str(&format!(
            "aws-sdk-js/1.0.27 KiroIDE-0.7.45-{fingerprint}"
        ))
        .unwrap(),
    );
    headers.insert(
        "x-amzn-codewhisperer-optout",
        HeaderValue::from_static("true"),
    );
    headers.insert(
        "x-amzn-kiro-agent-mode",
        HeaderValue::from_static("vibe"),
    );
    headers.insert(
        HeaderName::from_static("amz-sdk-invocation-id"),
        HeaderValue::from_str(&Uuid::new_v4().to_string()).unwrap(),
    );
    headers.insert(
        HeaderName::from_static("amz-sdk-request"),
        HeaderValue::from_static("attempt=1; max=3"),
    );

    headers
}

// ---------------------------------------------------------------------------
// KiroAuthManager – token refresh logic
// ---------------------------------------------------------------------------

/// Cached access-token state.
struct TokenState {
    access_token: String,
    expires_at: DateTime<Utc>,
}

/// Configuration for connecting to the Kiro API gateway.
#[derive(Clone, Debug)]
pub struct KiroClientConfig {
    /// Refresh token for obtaining access tokens.
    pub refresh_token: String,
    /// AWS region (default: us-east-1).
    pub region: String,
    /// Profile ARN for AWS CodeWhisperer (optional).
    pub profile_arn: Option<String>,
}

/// Manages the Kiro access-token lifecycle.
///
/// The refresh endpoint is `prod.{region}.auth.desktop.kiro.dev/refreshToken`.
/// Tokens are refreshed proactively when they are about to expire, matching
/// the behaviour of the kiro-aws Python reference.
struct KiroAuthManager {
    http: reqwest::Client,
    config: KiroClientConfig,
    token: RwLock<Option<TokenState>>,
    fingerprint: String,
}

impl KiroAuthManager {
    fn new(config: KiroClientConfig) -> Self {
        Self {
            http: reqwest::Client::new(),
            config,
            token: RwLock::new(None),
            fingerprint: machine_fingerprint(),
        }
    }

    /// Get a valid access token, refreshing if necessary.
    async fn get_access_token(&self) -> Result<String, ProviderError> {
        // Fast path: token is still valid
        {
            let guard = self.token.read().await;
            if let Some(ref ts) = *guard {
                let threshold =
                    Utc::now() + chrono::Duration::from_std(TOKEN_REFRESH_THRESHOLD).unwrap();
                if ts.expires_at > threshold {
                    return Ok(ts.access_token.clone());
                }
            }
        }

        // Slow path: refresh
        self.refresh().await
    }

    async fn refresh(&self) -> Result<String, ProviderError> {
        let url = kiro_refresh_url(&self.config.region);
        let payload = serde_json::json!({
            "refreshToken": self.config.refresh_token
        });
        let ua = format!("KiroIDE-0.7.45-{}", self.fingerprint);

        debug!("Refreshing Kiro access token via {url}");

        let resp = self
            .http
            .post(&url)
            .header("content-type", "application/json")
            .header("user-agent", &ua)
            .json(&payload)
            .send()
            .await
            .map_err(|e| {
                ProviderError::Other(format!("Kiro token refresh HTTP error: {e}"))
            })?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(ProviderError::NoToken {
                provider: format!(
                    "kiro: token refresh failed (HTTP {status}): {body}"
                ),
            });
        }

        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct RefreshResponse {
            access_token: String,
            #[serde(default = "default_expires_in")]
            expires_in: i64,
        }
        fn default_expires_in() -> i64 {
            3600
        }

        let data: RefreshResponse = resp.json().await.map_err(|e| {
            ProviderError::Other(format!("Kiro token refresh parse error: {e}"))
        })?;

        let expires_at = Utc::now()
            + chrono::Duration::seconds(data.expires_in)
            - chrono::Duration::from_std(EXPIRY_SAFETY_MARGIN).unwrap();

        let token = data.access_token.clone();
        {
            let mut guard = self.token.write().await;
            *guard = Some(TokenState {
                access_token: data.access_token,
                expires_at,
            });
        }
        info!("Kiro access token refreshed, expires at {expires_at}");
        Ok(token)
    }
}

// ---------------------------------------------------------------------------
// KiroClient – lightweight HTTP client for the Kiro API
// ---------------------------------------------------------------------------

/// Lightweight HTTP wrapper for the Kiro API.
///
/// The transport layer is fully public so that callers can build on it
/// for raw Kiro API endpoints (e.g. `ListAvailableModels`,
/// `generateAssistantResponse` with a raw payload) without going through
/// the `KiroProvider` / `KiroTransformer` abstraction.
pub struct KiroClient {
    http: reqwest::Client,
    auth: KiroAuthManager,
}

impl KiroClient {
    /// Create a new `KiroClient` with the given configuration.
    pub fn new(config: KiroClientConfig) -> Self {
        Self {
            http: reqwest::Client::new(),
            auth: KiroAuthManager::new(config),
        }
    }

    // -- Accessors ---------------------------------------------------------

    /// Current AWS region.
    pub fn region(&self) -> &str {
        &self.auth.config.region
    }

    /// Current profile ARN, if any.
    pub fn profile_arn(&self) -> Option<&str> {
        self.auth.config.profile_arn.as_deref()
    }

    /// Get a valid access token (refreshing if necessary).
    pub async fn access_token(&self) -> Result<String, ProviderError> {
        self.auth.get_access_token().await
    }

    // -- generateAssistantResponse convenience methods ---------------------

    /// Send to `generateAssistantResponse` (non-streaming) and return the
    /// full SSE body as a string.
    ///
    /// The Kiro endpoint always returns SSE, even when the caller doesn't
    /// want streaming – this method collects the entire body.
    pub async fn send_request(&self, body: &Value) -> Result<String, ProviderError> {
        let token = self.auth.get_access_token().await?;
        let url = generate_assistant_response_url(
            &self.auth.config.region,
            self.auth.config.profile_arn.as_deref(),
        );
        let headers = kiro_headers(&token, &self.auth.fingerprint);

        let resp = self
            .http
            .post(&url)
            .headers(headers)
            .json(body)
            .send()
            .await
            .map_err(ProviderError::Http)?;

        let status = resp.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let retry_after = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .map(Duration::from_secs);
            return Err(ProviderError::RateLimited {
                retry_after_secs: retry_after.map(|d| d.as_secs()).unwrap_or(60),
                retry_after,
            });
        }

        if status == reqwest::StatusCode::UNAUTHORIZED
            || status == reqwest::StatusCode::FORBIDDEN
        {
            return Err(ProviderError::NoToken {
                provider: "kiro".to_string(),
            });
        }

        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            return Err(ProviderError::Api {
                status: status.as_u16(),
                message: body_text,
            });
        }

        resp.text()
            .await
            .map_err(|e| ProviderError::ResponseParsing(format!("Failed to read Kiro response body: {e}")))
    }

    /// Send to `generateAssistantResponse` (streaming) and return a stream
    /// of SSE `data:` lines.
    pub async fn send_request_stream(
        &self,
        body: &Value,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<String, ProviderError>> + Send>>, ProviderError>
    {
        let token = self.auth.get_access_token().await?;
        let url = generate_assistant_response_url(
            &self.auth.config.region,
            self.auth.config.profile_arn.as_deref(),
        );
        let mut headers = kiro_headers(&token, &self.auth.fingerprint);
        // Prevent CLOSE_WAIT connection leak (kiro-gateway issue #38)
        headers.insert("connection", "close".parse().unwrap());

        let resp = self
            .http
            .post(&url)
            .headers(headers)
            .json(body)
            .send()
            .await
            .map_err(ProviderError::Http)?;

        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            return Err(ProviderError::Api {
                status: status.as_u16(),
                message: body_text,
            });
        }

        // Parse SSE: lines starting with "data: " contain event data
        let byte_stream = resp.bytes_stream();
        let line_stream = byte_stream
            .map(|result| {
                result.map_err(|e| ProviderError::Stream(format!("Stream read error: {e}")))
            })
            .flat_map(|result| {
                let lines: Vec<Result<String, ProviderError>> = match result {
                    Ok(bytes) => {
                        let text = String::from_utf8_lossy(&bytes);
                        text.lines()
                            .filter_map(|line| {
                                let line = line.trim();
                                if let Some(data) = line.strip_prefix("data: ") {
                                    if data == "[DONE]" {
                                        None
                                    } else {
                                        Some(Ok(data.to_string()))
                                    }
                                } else {
                                    None
                                }
                            })
                            .collect()
                    }
                    Err(e) => vec![Err(e)],
                };
                futures::stream::iter(lines)
            });

        Ok(Box::pin(line_stream))
    }

    // -- Raw transport (arbitrary endpoint) --------------------------------

    /// POST an arbitrary JSON payload to a fully-qualified Kiro API URL.
    ///
    /// Headers (auth, user-agent, etc.) are added automatically. This is
    /// the building block for adding raw Kiro API calls such as
    /// `ListAvailableModels` without modifying the provider layer.
    pub async fn raw_post(
        &self,
        url: &str,
        body: &Value,
    ) -> Result<reqwest::Response, ProviderError> {
        let token = self.auth.get_access_token().await?;
        let headers = kiro_headers(&token, &self.auth.fingerprint);
        self.http
            .post(url)
            .headers(headers)
            .json(body)
            .send()
            .await
            .map_err(ProviderError::Http)
    }

    /// POST an arbitrary JSON payload with streaming response.
    ///
    /// Returns the raw `reqwest::Response` so the caller can consume the
    /// byte stream however it likes.
    pub async fn raw_post_stream(
        &self,
        url: &str,
        body: &Value,
    ) -> Result<reqwest::Response, ProviderError> {
        let token = self.auth.get_access_token().await?;
        let mut headers = kiro_headers(&token, &self.auth.fingerprint);
        headers.insert("connection", "close".parse().unwrap());
        self.http
            .post(url)
            .headers(headers)
            .json(body)
            .send()
            .await
            .map_err(ProviderError::Http)
    }

    // -- Helpers -----------------------------------------------------------

    /// Build the `generateAssistantResponse` URL for this client's
    /// configured region and profile ARN.
    pub fn generate_assistant_response_url(&self) -> String {
        generate_assistant_response_url(
            &self.auth.config.region,
            self.auth.config.profile_arn.as_deref(),
        )
    }

    /// Simple health check – verify we can get a token.
    pub async fn health_check(&self) -> bool {
        self.auth.get_access_token().await.is_ok()
    }
}

// ---------------------------------------------------------------------------
// KiroProvider
// ---------------------------------------------------------------------------

/// LLM provider that communicates through the Kiro API.
///
/// Handles authentication via refresh token and API communication using
/// `reqwest`. Format conversion between OpenAI types and Anthropic Messages
/// types is delegated to `KiroTransformer`.
pub struct KiroProvider {
    client: Arc<KiroClient>,
    transformer: KiroTransformer,
}

impl KiroProvider {
    /// Create a new Kiro provider wrapping an already-built KiroClient.
    pub fn new(client: KiroClient) -> Self {
        Self {
            client: Arc::new(client),
            transformer: KiroTransformer::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// LlmProvider implementation
// ---------------------------------------------------------------------------

impl LlmProvider for KiroProvider {
    fn id(&self) -> &str {
        "kiro"
    }

    fn name(&self) -> &str {
        "Kiro (Amazon Q)"
    }

    fn models(&self) -> Vec<String> {
        self.transformer.supported_models()
    }

    fn supports_model(&self, model: &str) -> bool {
        self.transformer.supports_model(model)
    }

    fn chat(
        &self,
        request: &ChatRequest,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<ChatResponse, ProviderError>> + Send + '_>>
    {
        let request = request.clone();
        Box::pin(async move {
            // 1. Transform: OpenAI → Anthropic JSON via KiroTransformer
            let body: Value = self.transformer.transform_request(&request)?;
            debug!(body = %body, "Kiro request body");

            // 2. Send via HTTP – the Kiro endpoint always returns SSE,
            //    even for non-streaming requests.
            let sse_body = self.client.send_request(&body).await?;

            // 3. Parse the SSE body into JSON events via KiroStreamState
            let mut state = self.transformer.new_stream_state(&request.model);
            let mut final_chunk: Option<ChatChunk> = None;
            for line in sse_body.lines() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let data = if let Some(d) = line.strip_prefix("data: ") {
                    d
                } else {
                    line
                };
                if data == "[DONE]" {
                    continue;
                }
                if let Ok(Some(chunk)) = state.process_event(data) {
                    final_chunk = Some(chunk);
                }
            }

            // 4. Convert the accumulated stream chunks to a ChatResponse
            if let Some(chunk) = final_chunk {
                let meta = ProviderResponseMeta {
                    provider: "kiro".to_string(),
                    model: request.model.clone(),
                    created: chrono::Utc::now().timestamp(),
                    ..Default::default()
                };
                // Build a ChatResponse from the final accumulated chunk
                Ok(ChatResponse {
                    id: chunk.id,
                    object: "chat.completion".to_string(),
                    created: meta.created,
                    model: meta.model,
                    choices: chunk
                        .choices
                        .into_iter()
                        .map(|c| Choice {
                            index: c.index,
                            message: ResponseMessage {
                                role: "assistant".to_string(),
                                content: c.delta.content,
                                reasoning_content: c.delta.reasoning_content,
                                tool_calls: c.delta.tool_calls,
                            },
                            finish_reason: c.finish_reason,
                        })
                        .collect(),
                    usage: chunk.usage.unwrap_or_default(),
                })
            } else {
                Err(ProviderError::ResponseParsing(
                    "Kiro SSE response contained no usable events".to_string(),
                ))
            }
        })
    }

    fn stream_chat(
        &self,
        request: &ChatRequest,
    ) -> Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<
                        Pin<Box<dyn Stream<Item = Result<ChatChunk, ProviderError>> + Send>>,
                        ProviderError,
                    >,
                > + Send
                + '_,
        >,
    > {
        let request = request.clone();
        Box::pin(async move {
            // 1. Transform: OpenAI → Anthropic JSON via KiroTransformer
            let body: Value = self.transformer.transform_request(&request)?;

            // 2. Send streaming request
            let sse_stream = self.client.send_request_stream(&body).await?;

            // 3. Create a stream state processor from KiroTransformer
            let model = request.model.clone();
            let mut stream_state = self.transformer.new_stream_state(&model);

            // 4. Map SSE data lines through KiroStreamState
            let event_stream = sse_stream.filter_map(move |result| {
                let chunk = match result {
                    Ok(data) => match stream_state.process_event(&data) {
                        Ok(Some(chunk)) => Some(Ok(chunk)),
                        Ok(None) => None,
                        Err(e) => Some(Err(e)),
                    },
                    Err(e) => Some(Err(e)),
                };
                async move { chunk }
            });

            Ok(Box::pin(event_stream)
                as Pin<Box<dyn Stream<Item = Result<ChatChunk, ProviderError>> + Send>>)
        })
    }

    fn health_check(&self) -> Pin<Box<dyn std::future::Future<Output = bool> + Send + '_>> {
        Box::pin(async move { self.client.health_check().await })
    }

    fn pricing(&self) -> Vec<ModelPricing> {
        crate::providers::cost::CostCalculator::all()
            .into_iter()
            .filter(|p| p.provider == "kiro")
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_supported_models() {
        let transformer = KiroTransformer::new();
        let models = transformer.supported_models();
        for m in &models {
            assert!(m.starts_with("kiro:"), "Model {m} should start with kiro:");
        }
    }

    #[test]
    fn test_transform_request_roundtrip() {
        let transformer = KiroTransformer::new();
        let req = ChatRequest {
            model: "kiro:claude-sonnet-4".into(),
            messages: vec![
                ChatMessage {
                    role: MessageRole::System,
                    content: Some(MessageContent::Text("You are helpful.".into())),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
                ChatMessage {
                    role: MessageRole::User,
                    content: Some(MessageContent::Text("Hello".into())),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
            ],
            temperature: Some(0.7),
            max_tokens: Some(4096),
            stream: false,
            top_p: None,
            stop: None,
            tools: None,
            tool_choice: None,
            stream_options: None,
        };

        let body = transformer.transform_request(&req).unwrap();
        assert_eq!(body["model"], "claude-sonnet-4");
        assert_eq!(body["max_tokens"], 4096);
        assert_eq!(body["temperature"], 0.7);
        assert_eq!(body["messages"].as_array().unwrap().len(), 1);
        assert!(body["system"].is_string() || body["system"].is_array());
    }

    #[test]
    fn test_transform_response_roundtrip() {
        let transformer = KiroTransformer::new();
        let resp = serde_json::json!({
            "id": "msg_123",
            "type": "message",
            "role": "assistant",
            "content": [{"type": "text", "text": "Hello there!"}],
            "model": "claude-sonnet-4",
            "stop_reason": "end_turn",
            "stop_sequence": null,
            "usage": {"input_tokens": 10, "output_tokens": 5}
        });

        let meta = ProviderResponseMeta {
            provider: "kiro".to_string(),
            model: "kiro:claude-sonnet-4".to_string(),
            created: 1700000000,
            ..Default::default()
        };
        let chat_resp = transformer.transform_response(resp, &meta).unwrap();

        assert_eq!(chat_resp.id, "msg_123");
        assert_eq!(chat_resp.model, "kiro:claude-sonnet-4");
        assert_eq!(chat_resp.created, 1700000000);
        assert_eq!(
            chat_resp.choices[0].message.content,
            Some("Hello there!".to_string())
        );
        assert_eq!(chat_resp.choices[0].finish_reason, Some("stop".to_string()));
        assert_eq!(chat_resp.usage.prompt_tokens, 10);
        assert_eq!(chat_resp.usage.completion_tokens, 5);
        assert_eq!(chat_resp.usage.total_tokens, 15);
    }

    #[test]
    fn test_transform_response_tool_use_roundtrip() {
        let transformer = KiroTransformer::new();
        let resp = serde_json::json!({
            "id": "msg_456",
            "type": "message",
            "role": "assistant",
            "content": [{
                "type": "tool_use",
                "id": "toolu_1",
                "name": "search",
                "input": {"q": "test"}
            }],
            "model": "claude-sonnet-4",
            "stop_reason": "tool_use",
            "stop_sequence": null,
            "usage": {"input_tokens": 0, "output_tokens": 0}
        });

        let meta = ProviderResponseMeta {
            provider: "kiro".to_string(),
            model: "kiro:claude-sonnet-4".to_string(),
            created: 1700000000,
            ..Default::default()
        };
        let chat_resp = transformer.transform_response(resp, &meta).unwrap();

        let tc = chat_resp.choices[0].message.tool_calls.as_ref().unwrap();
        assert_eq!(tc.len(), 1);
        assert_eq!(tc[0].id, "toolu_1");
        assert_eq!(tc[0].function.name, "search");
        assert_eq!(tc[0].function.arguments, r#"{"q":"test"}"#);
        assert_eq!(
            chat_resp.choices[0].finish_reason,
            Some("tool_calls".to_string())
        );
    }

    #[test]
    fn test_transform_request_with_tools_roundtrip() {
        let transformer = KiroTransformer::new();
        let req = ChatRequest {
            model: "kiro:auto".into(),
            messages: vec![ChatMessage {
                role: MessageRole::User,
                content: Some(MessageContent::Text("Search for rust".into())),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            }],
            temperature: None,
            max_tokens: None,
            stream: false,
            top_p: None,
            stop: None,
            tools: Some(vec![Tool {
                r#type: "function".to_string(),
                function: FunctionDef {
                    name: "search".to_string(),
                    description: Some("Search the web".to_string()),
                    parameters: Some(serde_json::json!({
                        "type": "object",
                        "properties": {
                            "query": {"type": "string"}
                        }
                    })),
                },
            }]),
            tool_choice: Some(serde_json::json!("auto")),
            stream_options: None,
        };

        let body = transformer.transform_request(&req).unwrap();

        let tools = body["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "search");
        assert_eq!(tools[0]["description"], "Search the web");
    }

    #[test]
    fn test_stream_event_roundtrip() {
        let transformer = KiroTransformer::new();
        let mut state = transformer.new_stream_state("kiro:auto");

        let event_json = serde_json::json!({
            "type": "message_start",
            "message": {
                "id": "msg_1",
                "type": "message",
                "role": "assistant",
                "model": "claude-sonnet-4",
                "usage": {"input_tokens": 0, "output_tokens": 0}
            }
        });
        let json_str = serde_json::to_string(&event_json).unwrap();
        let chunk = state.process_event(&json_str).unwrap();
        assert!(chunk.is_some());
        let chunk = chunk.unwrap();
        assert_eq!(chunk.choices[0].delta.role, Some("assistant".to_string()));

        let event_json = serde_json::json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": {"type": "text_delta", "text": "Hello"}
        });
        let json_str = serde_json::to_string(&event_json).unwrap();
        let chunk = state.process_event(&json_str).unwrap().unwrap();
        assert_eq!(chunk.choices[0].delta.content, Some("Hello".to_string()));
    }

    #[test]
    fn test_url_encode_arn() {
        let arn = "arn:aws:q:us-east-1:123456789012:profile/abc-123";
        let encoded = url_encode(arn);
        assert!(encoded.contains("%3A"));
        assert!(encoded.contains("%2F"));
        assert!(!encoded.contains(':'));
        assert!(!encoded.contains('/'));
    }

    #[test]
    fn test_generate_assistant_response_url_with_arn() {
        let url = generate_assistant_response_url(
            "us-east-1",
            Some("arn:aws:q:us-east-1:123:profile/x"),
        );
        assert!(url.starts_with("https://q.us-east-1.amazonaws.com/"));
        assert!(url.contains("origin=AI_EDITOR"));
        assert!(url.contains("profileArn=arn%3Aaws"));
    }

    #[test]
    fn test_generate_assistant_response_url_without_arn() {
        let url = generate_assistant_response_url("eu-west-1", None);
        assert!(url.starts_with("https://q.eu-west-1.amazonaws.com/"));
        assert!(url.contains("origin=AI_EDITOR"));
        assert!(!url.contains("profileArn"));
    }
}
