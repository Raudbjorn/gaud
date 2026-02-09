//! GitHub Copilot Provider
//!
//! Routes requests to the GitHub Copilot Chat Completions API, which natively
//! accepts OpenAI-format payloads. Minimal conversion is needed.

use std::pin::Pin;

use futures::stream::{self, StreamExt};
use futures::Stream;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::providers::types::*;
use crate::providers::{LlmProvider, ProviderError, TokenStorage};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const API_ENDPOINT: &str = "https://api.githubcopilot.com/chat/completions";

const SUPPORTED_MODELS: &[&str] = &[
    "gpt-4o",
    "gpt-4-turbo",
    "o1",
    "o3-mini",
];

// ---------------------------------------------------------------------------
// Copilot API request / response (OpenAI-compatible)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct CopilotRequest {
    model: String,
    messages: Vec<CopilotMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stop: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<serde_json::Value>,
    stream: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct CopilotMessage {
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CopilotResponse {
    id: String,
    object: String,
    created: i64,
    model: String,
    choices: Vec<CopilotChoice>,
    usage: Option<CopilotUsage>,
}

#[derive(Debug, Deserialize)]
struct CopilotChoice {
    index: u32,
    message: CopilotResponseMessage,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CopilotResponseMessage {
    role: String,
    content: Option<String>,
    tool_calls: Option<Vec<CopilotToolCall>>,
}

#[derive(Debug, Deserialize)]
struct CopilotToolCall {
    id: String,
    r#type: String,
    function: CopilotFunctionCall,
}

#[derive(Debug, Deserialize)]
struct CopilotFunctionCall {
    name: String,
    arguments: String,
}

#[derive(Debug, Deserialize)]
struct CopilotUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
    total_tokens: u32,
}

// ---------------------------------------------------------------------------
// Copilot SSE chunk types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct CopilotStreamChunk {
    id: String,
    object: String,
    created: i64,
    model: String,
    choices: Vec<CopilotStreamChoice>,
    usage: Option<CopilotUsage>,
}

#[derive(Debug, Deserialize)]
struct CopilotStreamChoice {
    index: u32,
    delta: CopilotDelta,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CopilotDelta {
    role: Option<String>,
    content: Option<String>,
    tool_calls: Option<Vec<serde_json::Value>>,
}

// ---------------------------------------------------------------------------
// Copilot Provider
// ---------------------------------------------------------------------------

/// LLM provider that communicates with the GitHub Copilot Chat API.
pub struct CopilotProvider<T: TokenStorage> {
    http: Client,
    tokens: std::sync::Arc<T>,
}

impl<T: TokenStorage + 'static> CopilotProvider<T> {
    /// Create a new Copilot provider backed by the given token storage.
    pub fn new(tokens: std::sync::Arc<T>) -> Self {
        Self {
            http: Client::new(),
            tokens,
        }
    }

    /// Retrieve an access token or return an error.
    async fn get_token(&self) -> Result<String, ProviderError> {
        self.tokens
            .get_access_token("copilot")
            .await?
            .ok_or_else(|| ProviderError::NoToken("copilot".into()))
    }

    // -- conversion helpers --------------------------------------------------

    fn convert_request(request: &ChatRequest) -> CopilotRequest {
        let messages: Vec<CopilotMessage> = request
            .messages
            .iter()
            .map(|msg| {
                let role = match msg.role {
                    MessageRole::System => "system",
                    MessageRole::User => "user",
                    MessageRole::Assistant => "assistant",
                    MessageRole::Tool => "tool",
                };

                let content = msg.content.as_ref().map(|c| match c {
                    MessageContent::Text(t) => serde_json::Value::String(t.clone()),
                    MessageContent::Parts(parts) => {
                        let json_parts: Vec<serde_json::Value> = parts
                            .iter()
                            .map(|p| match p {
                                ContentPart::Text { text } => serde_json::json!({
                                    "type": "text",
                                    "text": text,
                                }),
                                ContentPart::ImageUrl { image_url } => serde_json::json!({
                                    "type": "image_url",
                                    "image_url": {
                                        "url": image_url.url,
                                        "detail": image_url.detail,
                                    },
                                }),
                            })
                            .collect();
                        serde_json::Value::Array(json_parts)
                    }
                });

                let tool_calls = msg.tool_calls.as_ref().map(|tcs| {
                    tcs.iter()
                        .map(|tc| {
                            serde_json::json!({
                                "id": tc.id,
                                "type": tc.r#type,
                                "function": {
                                    "name": tc.function.name,
                                    "arguments": tc.function.arguments,
                                }
                            })
                        })
                        .collect()
                });

                CopilotMessage {
                    role: role.into(),
                    content,
                    name: msg.name.clone(),
                    tool_calls,
                    tool_call_id: msg.tool_call_id.clone(),
                }
            })
            .collect();

        let stop = request.stop.as_ref().map(|s| match s {
            StopSequence::Single(s) => serde_json::Value::String(s.clone()),
            StopSequence::Multiple(v) => serde_json::to_value(v).unwrap_or_default(),
        });

        let tools = request.tools.as_ref().map(|ts| {
            ts.iter()
                .map(|t| {
                    serde_json::json!({
                        "type": t.r#type,
                        "function": {
                            "name": t.function.name,
                            "description": t.function.description,
                            "parameters": t.function.parameters,
                        }
                    })
                })
                .collect()
        });

        CopilotRequest {
            model: request.model.clone(),
            messages,
            temperature: request.temperature,
            max_tokens: request.max_tokens,
            top_p: request.top_p,
            stop,
            tools,
            tool_choice: request.tool_choice.clone(),
            stream: false,
        }
    }

    fn convert_response(api_resp: CopilotResponse) -> ChatResponse {
        let choices: Vec<Choice> = api_resp
            .choices
            .iter()
            .map(|c| {
                let tool_calls = c.message.tool_calls.as_ref().map(|tcs| {
                    tcs.iter()
                        .map(|tc| ToolCall {
                            id: tc.id.clone(),
                            r#type: tc.r#type.clone(),
                            function: FunctionCall {
                                name: tc.function.name.clone(),
                                arguments: tc.function.arguments.clone(),
                            },
                        })
                        .collect()
                });

                Choice {
                    index: c.index,
                    message: ResponseMessage {
                        role: c.message.role.clone(),
                        content: c.message.content.clone(),
                        tool_calls,
                    },
                    finish_reason: c.finish_reason.clone(),
                }
            })
            .collect();

        let usage = api_resp
            .usage
            .map(|u| Usage {
                prompt_tokens: u.prompt_tokens,
                completion_tokens: u.completion_tokens,
                total_tokens: u.total_tokens,
            })
            .unwrap_or_default();

        ChatResponse {
            id: api_resp.id,
            object: api_resp.object,
            created: api_resp.created,
            model: api_resp.model,
            choices,
            usage,
        }
    }
}

// ---------------------------------------------------------------------------
// LlmProvider implementation
// ---------------------------------------------------------------------------

impl<T: TokenStorage + 'static> LlmProvider for CopilotProvider<T> {
    fn id(&self) -> &str {
        "copilot"
    }

    fn name(&self) -> &str {
        "GitHub Copilot"
    }

    fn models(&self) -> Vec<String> {
        SUPPORTED_MODELS.iter().map(|s| s.to_string()).collect()
    }

    fn supports_model(&self, model: &str) -> bool {
        SUPPORTED_MODELS.iter().any(|m| *m == model)
    }

    fn chat(
        &self,
        request: &ChatRequest,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<ChatResponse, ProviderError>> + Send + '_>> {
        let request = request.clone();
        Box::pin(async move {
            let token = self.get_token().await?;
            let mut body = Self::convert_request(&request);
            body.stream = false;

            let resp = self
                .http
                .post(API_ENDPOINT)
                .bearer_auth(&token)
                .header("content-type", "application/json")
                .header("editor-version", "gaud/0.1.0")
                .header("copilot-integration-id", "gaud")
                .json(&body)
                .send()
                .await?;

            let status = resp.status();
            if !status.is_success() {
                let text = resp.text().await.unwrap_or_default();
                if status.as_u16() == 429 {
                    return Err(ProviderError::RateLimited {
                        retry_after_secs: 60,
                    });
                }
                return Err(ProviderError::Api {
                    status: status.as_u16(),
                    message: text,
                });
            }

            let api_resp: CopilotResponse = resp.json().await?;
            Ok(Self::convert_response(api_resp))
        })
    }

    fn stream_chat(
        &self,
        request: &ChatRequest,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<Pin<Box<dyn Stream<Item = Result<ChatChunk, ProviderError>> + Send>>, ProviderError>> + Send + '_>> {
        let request = request.clone();
        Box::pin(async move {
        let token = self.get_token().await?;
        let mut body = Self::convert_request(&request);
        body.stream = true;

        let resp = self
            .http
            .post(API_ENDPOINT)
            .bearer_auth(&token)
            .header("content-type", "application/json")
            .header("editor-version", "gaud/0.1.0")
            .header("copilot-integration-id", "gaud")
            .json(&body)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            if status.as_u16() == 429 {
                return Err(ProviderError::RateLimited {
                    retry_after_secs: 60,
                });
            }
            return Err(ProviderError::Api {
                status: status.as_u16(),
                message: text,
            });
        }

        let byte_stream = resp.bytes_stream();

        let event_stream = stream::unfold(
            (
                Box::pin(byte_stream.map(|r| r.map_err(|e| ProviderError::Stream(e.to_string())))),
                String::new(),
            ),
            |(mut inner, mut buf)| async move {
                loop {
                    while let Some(newline_pos) = buf.find('\n') {
                        let line = buf[..newline_pos].trim_end_matches('\r').to_string();
                        buf = buf[newline_pos + 1..].to_string();

                        if line.is_empty() {
                            continue;
                        }

                        if let Some(data) = line.strip_prefix("data: ") {
                            if data.trim() == "[DONE]" {
                                return None;
                            }

                            let chunk: CopilotStreamChunk = match serde_json::from_str(data) {
                                Ok(c) => c,
                                Err(e) => {
                                    debug!(error = %e, "Failed to parse Copilot SSE chunk");
                                    continue;
                                }
                            };

                            let choices: Vec<ChunkChoice> = chunk
                                .choices
                                .iter()
                                .map(|c| {
                                    let tool_calls = c.delta.tool_calls.as_ref().map(|tcs| {
                                        tcs.iter()
                                            .filter_map(|tc| {
                                                serde_json::from_value::<ToolCall>(tc.clone()).ok()
                                            })
                                            .collect()
                                    });
                                    ChunkChoice {
                                        index: c.index,
                                        delta: Delta {
                                            role: c.delta.role.clone(),
                                            content: c.delta.content.clone(),
                                            tool_calls,
                                        },
                                        finish_reason: c.finish_reason.clone(),
                                    }
                                })
                                .collect();

                            let usage = chunk.usage.map(|u| Usage {
                                prompt_tokens: u.prompt_tokens,
                                completion_tokens: u.completion_tokens,
                                total_tokens: u.total_tokens,
                            });

                            let chat_chunk = ChatChunk {
                                id: chunk.id,
                                object: chunk.object,
                                created: chunk.created,
                                model: chunk.model,
                                choices,
                                usage,
                            };

                            return Some((Ok(chat_chunk), (inner, buf)));
                        }
                    }

                    match inner.next().await {
                        Some(Ok(bytes)) => {
                            buf.push_str(&String::from_utf8_lossy(&bytes));
                        }
                        Some(Err(e)) => {
                            return Some((Err(e), (inner, buf)));
                        }
                        None => return None,
                    }
                }
            },
        );

        Ok(Box::pin(event_stream) as Pin<Box<dyn Stream<Item = Result<ChatChunk, ProviderError>> + Send>>)
        })
    }

    fn health_check(&self) -> Pin<Box<dyn std::future::Future<Output = bool> + Send + '_>> {
        Box::pin(async move { self.get_token().await.is_ok() })
    }

    fn pricing(&self) -> Vec<ModelPricing> {
        crate::providers::cost::CostDatabase::all()
            .into_iter()
            .filter(|p| p.provider == "copilot")
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct MockTokenStorage {
        token: Option<String>,
    }

    impl MockTokenStorage {
        fn with_token(token: &str) -> Self {
            Self {
                token: Some(token.into()),
            }
        }

        fn empty() -> Self {
            Self { token: None }
        }
    }

    impl TokenStorage for MockTokenStorage {
        async fn get_access_token(
            &self,
            _provider: &str,
        ) -> Result<Option<String>, ProviderError> {
            Ok(self.token.clone())
        }
    }

    #[test]
    fn test_id_and_name() {
        let p = CopilotProvider::new(Arc::new(MockTokenStorage::empty()));
        assert_eq!(p.id(), "copilot");
        assert_eq!(p.name(), "GitHub Copilot");
    }

    #[test]
    fn test_models_list() {
        let p = CopilotProvider::new(Arc::new(MockTokenStorage::empty()));
        let models = p.models();
        assert!(models.contains(&"gpt-4o".to_string()));
        assert!(models.contains(&"gpt-4-turbo".to_string()));
        assert!(models.contains(&"o1".to_string()));
        assert!(models.contains(&"o3-mini".to_string()));
    }

    #[test]
    fn test_supports_model() {
        let p = CopilotProvider::new(Arc::new(MockTokenStorage::empty()));
        assert!(p.supports_model("gpt-4o"));
        assert!(p.supports_model("o1"));
        assert!(!p.supports_model("claude-sonnet-4-20250514"));
    }

    #[tokio::test]
    async fn test_health_check_no_token() {
        let p = CopilotProvider::new(Arc::new(MockTokenStorage::empty()));
        assert!(!p.health_check().await);
    }

    #[tokio::test]
    async fn test_health_check_with_token() {
        let p = CopilotProvider::new(Arc::new(MockTokenStorage::with_token("test")));
        assert!(p.health_check().await);
    }

    #[tokio::test]
    async fn test_chat_no_token() {
        let p = CopilotProvider::new(Arc::new(MockTokenStorage::empty()));
        let req = ChatRequest {
            model: "gpt-4o".into(),
            messages: vec![ChatMessage {
                role: MessageRole::User,
                content: Some(MessageContent::Text("Hello".into())),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            }],
            temperature: None,
            max_tokens: None,
            stream: false,
            top_p: None,
            stop: None,
            tools: None,
            tool_choice: None,
        };
        let result = p.chat(&req).await;
        assert!(matches!(result, Err(ProviderError::NoToken(_))));
    }

    #[test]
    fn test_convert_request_basic() {
        let req = ChatRequest {
            model: "gpt-4o".into(),
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
                    content: Some(MessageContent::Text("Hi".into())),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
            ],
            temperature: Some(0.8),
            max_tokens: Some(4096),
            stream: false,
            top_p: None,
            stop: None,
            tools: None,
            tool_choice: None,
        };

        let converted = CopilotProvider::<MockTokenStorage>::convert_request(&req);
        assert_eq!(converted.model, "gpt-4o");
        assert_eq!(converted.messages.len(), 2);
        assert_eq!(converted.messages[0].role, "system");
        assert_eq!(converted.messages[1].role, "user");
        assert_eq!(converted.temperature, Some(0.8));
        assert_eq!(converted.max_tokens, Some(4096));
    }

    #[test]
    fn test_convert_response() {
        let api_resp = CopilotResponse {
            id: "chatcmpl-123".into(),
            object: "chat.completion".into(),
            created: 1700000000,
            model: "gpt-4o".into(),
            choices: vec![CopilotChoice {
                index: 0,
                message: CopilotResponseMessage {
                    role: "assistant".into(),
                    content: Some("Hello there!".into()),
                    tool_calls: None,
                },
                finish_reason: Some("stop".into()),
            }],
            usage: Some(CopilotUsage {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
            }),
        };

        let resp = CopilotProvider::<MockTokenStorage>::convert_response(api_resp);
        assert_eq!(resp.id, "chatcmpl-123");
        assert_eq!(resp.model, "gpt-4o");
        assert_eq!(resp.choices.len(), 1);
        assert_eq!(
            resp.choices[0].message.content,
            Some("Hello there!".to_string())
        );
        assert_eq!(
            resp.choices[0].finish_reason,
            Some("stop".to_string())
        );
        assert_eq!(resp.usage.prompt_tokens, 10);
        assert_eq!(resp.usage.completion_tokens, 5);
    }

    #[test]
    fn test_convert_response_tool_calls() {
        let api_resp = CopilotResponse {
            id: "chatcmpl-456".into(),
            object: "chat.completion".into(),
            created: 1700000000,
            model: "gpt-4o".into(),
            choices: vec![CopilotChoice {
                index: 0,
                message: CopilotResponseMessage {
                    role: "assistant".into(),
                    content: None,
                    tool_calls: Some(vec![CopilotToolCall {
                        id: "call_1".into(),
                        r#type: "function".into(),
                        function: CopilotFunctionCall {
                            name: "search".into(),
                            arguments: r#"{"q":"test"}"#.into(),
                        },
                    }]),
                },
                finish_reason: Some("tool_calls".into()),
            }],
            usage: Some(CopilotUsage {
                prompt_tokens: 20,
                completion_tokens: 10,
                total_tokens: 30,
            }),
        };

        let resp = CopilotProvider::<MockTokenStorage>::convert_response(api_resp);
        let tc = resp.choices[0].message.tool_calls.as_ref().unwrap();
        assert_eq!(tc.len(), 1);
        assert_eq!(tc[0].function.name, "search");
        assert_eq!(tc[0].function.arguments, r#"{"q":"test"}"#);
    }

    #[test]
    fn test_pricing_returns_copilot_models() {
        let p = CopilotProvider::new(Arc::new(MockTokenStorage::empty()));
        let pricing = p.pricing();
        assert!(!pricing.is_empty());
        for mp in &pricing {
            assert_eq!(mp.provider, "copilot");
            // Copilot models are subscription-based (free per-token).
            assert_eq!(mp.input_cost_per_million, 0.0);
        }
    }

    #[test]
    fn test_convert_request_with_stop() {
        let req = ChatRequest {
            model: "gpt-4o".into(),
            messages: vec![],
            temperature: None,
            max_tokens: None,
            stream: false,
            top_p: None,
            stop: Some(StopSequence::Single("END".into())),
            tools: None,
            tool_choice: None,
        };

        let converted = CopilotProvider::<MockTokenStorage>::convert_request(&req);
        assert_eq!(
            converted.stop,
            Some(serde_json::Value::String("END".into()))
        );
    }

    #[test]
    fn test_convert_request_multipart_content() {
        let req = ChatRequest {
            model: "gpt-4o".into(),
            messages: vec![ChatMessage {
                role: MessageRole::User,
                content: Some(MessageContent::Parts(vec![
                    ContentPart::Text {
                        text: "What is this?".into(),
                    },
                    ContentPart::ImageUrl {
                        image_url: ImageUrl {
                            url: "https://example.com/image.png".into(),
                            detail: Some("high".into()),
                        },
                    },
                ])),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            }],
            temperature: None,
            max_tokens: None,
            stream: false,
            top_p: None,
            stop: None,
            tools: None,
            tool_choice: None,
        };

        let converted = CopilotProvider::<MockTokenStorage>::convert_request(&req);
        let content = converted.messages[0].content.as_ref().unwrap();
        assert!(content.is_array());
        let arr = content.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["type"], "text");
        assert_eq!(arr[1]["type"], "image_url");
    }
}
