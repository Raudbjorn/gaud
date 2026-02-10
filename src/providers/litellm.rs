//! LiteLLM Provider
//!
//! Proxies chat requests to a LiteLLM instance via its OpenAI-compatible API.
//! Supports auto-discovery of available models and streaming responses.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use futures::stream::StreamExt;
use futures::Stream;
use reqwest::Client;
use tokio::sync::RwLock;
use tracing::{debug, warn};

use crate::providers::types::{
    ChatChunk, ChatRequest, ChatResponse, Choice, ChunkChoice, Delta,
    ResponseMessage, Usage,
};
use crate::providers::pricing::ModelPricing;
use crate::providers::{LlmProvider, ProviderError};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for the LiteLLM provider.
#[derive(Debug, Clone)]
pub struct LitellmConfig {
    /// Base URL of the LiteLLM proxy (e.g. `http://localhost:4000`).
    pub url: String,
    /// API key (master key or virtual key) for authentication.
    pub api_key: Option<String>,
    /// When true, models are fetched from `GET /v1/models` at startup and
    /// periodically thereafter. When false, only manually listed models are
    /// available.
    pub discover_models: bool,
    /// Manually listed model names (always available regardless of discovery).
    pub models: Vec<String>,
    /// Request timeout for chat completions.
    pub timeout_secs: u64,
}

// ---------------------------------------------------------------------------
// LiteLLM model response (from GET /v1/models)
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Deserialize)]
struct ModelsListResponse {
    data: Vec<ModelEntry>,
}

#[derive(Debug, serde::Deserialize)]
struct ModelEntry {
    id: String,
}

// ---------------------------------------------------------------------------
// OpenAI-compatible response types for deserialization
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Deserialize)]
struct OaiResponse {
    id: String,
    #[serde(default)]
    object: String,
    #[serde(default)]
    created: i64,
    #[serde(default)]
    model: String,
    #[serde(default)]
    choices: Vec<OaiChoice>,
    #[serde(default)]
    usage: Option<OaiUsage>,
}

#[derive(Debug, serde::Deserialize)]
struct OaiChoice {
    #[serde(default)]
    index: u32,
    #[serde(default)]
    message: Option<OaiMessage>,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct OaiMessage {
    #[serde(default)]
    role: String,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, serde::Deserialize)]
struct OaiUsage {
    #[serde(default)]
    prompt_tokens: u32,
    #[serde(default)]
    completion_tokens: u32,
    #[serde(default)]
    total_tokens: u32,
}

#[derive(Debug, serde::Deserialize)]
struct OaiStreamChunk {
    id: String,
    #[serde(default)]
    object: String,
    #[serde(default)]
    created: i64,
    #[serde(default)]
    model: String,
    #[serde(default)]
    choices: Vec<OaiStreamChoice>,
    #[serde(default)]
    usage: Option<OaiUsage>,
}

#[derive(Debug, serde::Deserialize)]
struct OaiStreamChoice {
    #[serde(default)]
    index: u32,
    #[serde(default)]
    delta: Option<OaiDelta>,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct OaiDelta {
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<serde_json::Value>>,
}

// ---------------------------------------------------------------------------
// LitellmProvider
// ---------------------------------------------------------------------------

/// LLM provider that proxies requests to a LiteLLM instance.
pub struct LitellmProvider {
    config: LitellmConfig,
    client: Client,
    /// Discovered models (populated by `discover()` or manually set).
    discovered_models: Arc<RwLock<Vec<String>>>,
}

impl LitellmProvider {
    /// Create a new LiteLLM provider and optionally discover available models.
    pub async fn new(config: LitellmConfig) -> Result<Self, ProviderError> {
        let client = Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .build()
            .map_err(|e| ProviderError::Other(format!("Failed to create HTTP client: {e}")))?;

        let initial_models: Vec<String> = config.models.clone();

        let provider = Self {
            config,
            client,
            discovered_models: Arc::new(RwLock::new(initial_models)),
        };

        if provider.config.discover_models {
            if let Err(e) = provider.discover().await {
                warn!(error = %e, "LiteLLM model discovery failed at startup, using manual list");
            }
        }

        Ok(provider)
    }

    /// Fetch models from the LiteLLM `/v1/models` endpoint and merge with
    /// the manual list.
    async fn discover(&self) -> Result<(), ProviderError> {
        let url = format!("{}/v1/models", self.config.url.trim_end_matches('/'));
        let mut req = self.client.get(&url);
        if let Some(ref key) = self.config.api_key {
            req = req.bearer_auth(key);
        }

        let resp = req.send().await.map_err(|e| {
            ProviderError::Other(format!("LiteLLM model discovery request failed: {e}"))
        })?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(ProviderError::Api {
                status,
                message: format!("Model discovery failed: {body}"),
            });
        }

        let body: ModelsListResponse = resp.json().await.map_err(|e| {
            ProviderError::Other(format!("Failed to parse model list: {e}"))
        })?;

        let mut models = self.discovered_models.write().await;

        // Merge discovered models with manual models (prefix with litellm:).
        let mut all: Vec<String> = self.config.models.clone();
        for entry in &body.data {
            let prefixed = format!("litellm:{}", entry.id);
            if !all.contains(&prefixed) {
                all.push(prefixed);
            }
            // Also keep the raw ID for direct matching.
            if !all.contains(&entry.id) {
                all.push(entry.id.clone());
            }
        }

        let count = all.len();
        *models = all;
        debug!(count, "LiteLLM models discovered");

        Ok(())
    }

    /// Build the request body for the LiteLLM proxy (OpenAI-compatible format).
    fn build_request_body(request: &ChatRequest) -> serde_json::Value {
        // Strip the `litellm:` prefix if present so LiteLLM receives the
        // native model name it recognizes.
        let model = request
            .model
            .strip_prefix("litellm:")
            .unwrap_or(&request.model);

        let mut body = serde_json::json!({
            "model": model,
            "messages": request.messages,
            "stream": request.stream,
        });

        if let Some(temp) = request.temperature {
            body["temperature"] = serde_json::json!(temp);
        }
        if let Some(max) = request.max_tokens {
            body["max_tokens"] = serde_json::json!(max);
        }
        if let Some(top_p) = request.top_p {
            body["top_p"] = serde_json::json!(top_p);
        }
        if let Some(ref stop) = request.stop {
            body["stop"] = serde_json::to_value(stop).unwrap_or_default();
        }
        if let Some(ref tools) = request.tools {
            body["tools"] = serde_json::to_value(tools).unwrap_or_default();
        }
        if let Some(ref tc) = request.tool_choice {
            body["tool_choice"] = tc.clone();
        }

        body
    }

    /// Convert an OAI response to our internal ChatResponse type.
    fn convert_response(oai: OaiResponse) -> ChatResponse {
        ChatResponse {
            id: oai.id,
            object: oai.object,
            created: oai.created,
            model: oai.model,
            choices: oai
                .choices
                .into_iter()
                .map(|c| {
                    let msg = c.message.unwrap_or(OaiMessage {
                        role: "assistant".to_string(),
                        content: None,
                        tool_calls: None,
                    });
                    Choice {
                        index: c.index,
                        message: ResponseMessage {
                            role: msg.role,
                            content: msg.content,
                            reasoning_content: None,
                            tool_calls: msg.tool_calls.and_then(|tc| {
                                serde_json::from_value(serde_json::Value::Array(tc)).ok()
                            }),
                        },
                        finish_reason: c.finish_reason,
                    }
                })
                .collect(),
            usage: oai
                .usage
                .map(|u| Usage {
                    prompt_tokens: u.prompt_tokens,
                    completion_tokens: u.completion_tokens,
                    total_tokens: u.total_tokens,
                    prompt_tokens_details: None,
                    completion_tokens_details: None,
                })
                .unwrap_or_default(),
        }
    }
}

impl LlmProvider for LitellmProvider {
    fn id(&self) -> &str {
        "litellm"
    }

    fn name(&self) -> &str {
        "LiteLLM Proxy"
    }

    fn models(&self) -> Vec<String> {
        // Return a snapshot of the discovered models. We use try_read to
        // avoid blocking; if the lock is held we return the manual list.
        match self.discovered_models.try_read() {
            Ok(models) => models.clone(),
            Err(_) => self.config.models.clone(),
        }
    }

    fn supports_model(&self, model: &str) -> bool {
        // If discover_models is enabled, we match any model with the
        // `litellm:` prefix (LiteLLM handles routing internally).
        if model.starts_with("litellm:") {
            return true;
        }

        // Also check the discovered model list for direct matches.
        match self.discovered_models.try_read() {
            Ok(models) => models.iter().any(|m| m == model),
            Err(_) => self.config.models.iter().any(|m| m == model),
        }
    }

    fn chat(
        &self,
        request: &ChatRequest,
    ) -> Pin<Box<dyn Future<Output = Result<ChatResponse, ProviderError>> + Send + '_>> {
        let request = request.clone();
        Box::pin(async move {
            let url = format!(
                "{}/v1/chat/completions",
                self.config.url.trim_end_matches('/')
            );
            let body = Self::build_request_body(&request);

            let mut req = self.client.post(&url).json(&body);
            if let Some(ref key) = self.config.api_key {
                req = req.bearer_auth(key);
            }

            let resp = req.send().await.map_err(|e| {
                ProviderError::Other(format!("LiteLLM request failed: {e}"))
            })?;

            let status = resp.status();
            if !status.is_success() {
                let code = status.as_u16();
                let body = resp.text().await.unwrap_or_default();
                return Err(ProviderError::Api {
                    status: code,
                    message: body,
                });
            }

            let oai: OaiResponse = resp.json().await.map_err(|e| {
                ProviderError::Other(format!("Failed to parse LiteLLM response: {e}"))
            })?;

            Ok(Self::convert_response(oai))
        })
    }

    fn stream_chat(
        &self,
        request: &ChatRequest,
    ) -> Pin<
        Box<
            dyn Future<
                    Output = Result<
                        Pin<Box<dyn Stream<Item = Result<ChatChunk, ProviderError>> + Send>>,
                        ProviderError,
                    >,
                > + Send
                + '_,
        >,
    > {
        let mut request = request.clone();
        request.stream = true;
        Box::pin(async move {
            let url = format!(
                "{}/v1/chat/completions",
                self.config.url.trim_end_matches('/')
            );
            let body = Self::build_request_body(&request);

            let mut req = self.client.post(&url).json(&body);
            if let Some(ref key) = self.config.api_key {
                req = req.bearer_auth(key);
            }

            let resp = req.send().await.map_err(|e| {
                ProviderError::Other(format!("LiteLLM stream request failed: {e}"))
            })?;

            let status = resp.status();
            if !status.is_success() {
                let code = status.as_u16();
                let body = resp.text().await.unwrap_or_default();
                return Err(ProviderError::Api {
                    status: code,
                    message: body,
                });
            }

            let byte_stream = resp.bytes_stream();

            let stream = byte_stream
                .map(|chunk_result| match chunk_result {
                    Ok(bytes) => {
                        let text = String::from_utf8_lossy(&bytes);
                        let mut chunks = Vec::new();

                        for line in text.lines() {
                            let line = line.trim();
                            if line.is_empty() || line == ":" {
                                continue;
                            }
                            let data = line.strip_prefix("data: ").unwrap_or(line);
                            if data == "[DONE]" {
                                continue;
                            }
                            match serde_json::from_str::<OaiStreamChunk>(data) {
                                Ok(oai) => {
                                    chunks.push(Ok(ChatChunk {
                                        id: oai.id,
                                        object: oai.object,
                                        created: oai.created,
                                        model: oai.model,
                                        choices: oai
                                            .choices
                                            .into_iter()
                                            .map(|c| {
                                                let delta =
                                                    c.delta.unwrap_or(OaiDelta {
                                                        role: None,
                                                        content: None,
                                                        tool_calls: None,
                                                    });
                                                ChunkChoice {
                                                    index: c.index,
                                                    delta: Delta {
                                                        role: delta.role,
                                                        content: delta.content,
                                                        reasoning_content: None,
                                                        tool_calls: delta
                                                            .tool_calls
                                                            .and_then(|tc| {
                                                                serde_json::from_value(
                                                                    serde_json::Value::Array(tc),
                                                                )
                                                                .ok()
                                                            }),
                                                    },
                                                    finish_reason: c.finish_reason,
                                                }
                                            })
                                            .collect(),
                                        usage: oai.usage.map(|u| Usage {
                                            prompt_tokens: u.prompt_tokens,
                                            completion_tokens: u.completion_tokens,
                                            total_tokens: u.total_tokens,
                                            prompt_tokens_details: None,
                                            completion_tokens_details: None,
                                        }),
                                    }));
                                }
                                Err(e) => {
                                    debug!(data = data, error = %e, "Skipping unparseable SSE line");
                                }
                            }
                        }

                        futures::stream::iter(chunks)
                    }
                    Err(e) => futures::stream::iter(vec![Err(ProviderError::Stream(
                        format!("LiteLLM stream error: {e}"),
                    ))]),
                })
                .flatten();

            Ok(
                Box::pin(stream)
                    as Pin<Box<dyn Stream<Item = Result<ChatChunk, ProviderError>> + Send>>,
            )
        })
    }

    fn health_check(&self) -> Pin<Box<dyn Future<Output = bool> + Send + '_>> {
        Box::pin(async move {
            let url = format!(
                "{}/health/liveliness",
                self.config.url.trim_end_matches('/')
            );
            let mut req = self.client.get(&url);
            if let Some(ref key) = self.config.api_key {
                req = req.bearer_auth(key);
            }

            match req.timeout(Duration::from_secs(5)).send().await {
                Ok(resp) => resp.status().is_success(),
                Err(_) => false,
            }
        })
    }

    fn pricing(&self) -> Vec<ModelPricing> {
        // LiteLLM handles its own pricing/budgets; we don't duplicate that
        // data here. Return an empty list.
        vec![]
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_request_body_strips_prefix() {
        let request = ChatRequest {
            model: "litellm:gpt-4o".to_string(),
            messages: vec![],
            temperature: Some(0.7),
            max_tokens: Some(1000),
            stream: false,
            top_p: None,
            stop: None,
            tools: None,
            tool_choice: None,
            stream_options: None,
        };

        let body = LitellmProvider::build_request_body(&request);
        assert_eq!(body["model"], "gpt-4o");
        let temp = body["temperature"].as_f64().unwrap();
        assert!((temp - 0.7).abs() < 0.001, "temperature was {temp}");
        assert_eq!(body["max_tokens"], 1000);
    }

    #[test]
    fn test_build_request_body_no_prefix() {
        let request = ChatRequest {
            model: "gpt-4".to_string(),
            messages: vec![],
            temperature: None,
            max_tokens: None,
            stream: true,
            top_p: None,
            stop: None,
            tools: None,
            tool_choice: None,
            stream_options: None,
        };

        let body = LitellmProvider::build_request_body(&request);
        assert_eq!(body["model"], "gpt-4");
        assert_eq!(body["stream"], true);
        assert!(body.get("temperature").is_none());
    }

    #[test]
    fn test_convert_response() {
        let oai = OaiResponse {
            id: "chatcmpl-123".to_string(),
            object: "chat.completion".to_string(),
            created: 1700000000,
            model: "gpt-4o".to_string(),
            choices: vec![OaiChoice {
                index: 0,
                message: Some(OaiMessage {
                    role: "assistant".to_string(),
                    content: Some("Hello!".to_string()),
                    tool_calls: None,
                }),
                finish_reason: Some("stop".to_string()),
            }],
            usage: Some(OaiUsage {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
            }),
        };

        let resp = LitellmProvider::convert_response(oai);
        assert_eq!(resp.id, "chatcmpl-123");
        assert_eq!(resp.choices.len(), 1);
        assert_eq!(
            resp.choices[0].message.content.as_deref(),
            Some("Hello!")
        );
        assert_eq!(resp.usage.total_tokens, 15);
    }

    #[tokio::test]
    async fn test_litellm_provider_models_with_manual_list() {
        let config = LitellmConfig {
            url: "http://localhost:4000".to_string(),
            api_key: None,
            discover_models: false,
            models: vec![
                "litellm:gpt-4o".to_string(),
                "litellm:claude-sonnet-4".to_string(),
            ],
            timeout_secs: 30,
        };

        let provider = LitellmProvider {
            config,
            client: Client::new(),
            discovered_models: Arc::new(RwLock::new(vec![
                "litellm:gpt-4o".to_string(),
                "litellm:claude-sonnet-4".to_string(),
            ])),
        };

        assert_eq!(provider.id(), "litellm");
        assert_eq!(provider.name(), "LiteLLM Proxy");
        assert_eq!(provider.models().len(), 2);
        assert!(provider.supports_model("litellm:gpt-4o"));
        assert!(provider.supports_model("litellm:anything-goes"));
        assert!(!provider.supports_model("gpt-4o"));
    }

    #[test]
    fn test_litellm_prefix_matching() {
        // Any model with litellm: prefix should be supported.
        let config = LitellmConfig {
            url: "http://localhost:4000".to_string(),
            api_key: None,
            discover_models: false,
            models: vec![],
            timeout_secs: 30,
        };

        let provider = LitellmProvider {
            config,
            client: Client::new(),
            discovered_models: Arc::new(RwLock::new(vec![])),
        };

        assert!(provider.supports_model("litellm:gpt-4o"));
        assert!(provider.supports_model("litellm:claude-sonnet-4"));
        assert!(provider.supports_model("litellm:anthropic/claude-3"));
        assert!(!provider.supports_model("openai:gpt-4o"));
    }
}
