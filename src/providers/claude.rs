//! Claude (Anthropic) Provider
//!
//! Converts OpenAI-format requests into Anthropic Messages API format,
//! sends them to `https://api.anthropic.com/v1/messages`, and converts the
//! response back to OpenAI format.

use std::pin::Pin;

use futures::stream::{self, StreamExt, TryStreamExt};
use futures::Stream;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::providers::types::*;
use crate::providers::{LlmProvider, ProviderError, TokenStorage};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const API_BASE: &str = "https://api.anthropic.com/v1";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_MAX_TOKENS: u32 = 8_192;

const SUPPORTED_MODELS: &[&str] = &[
    "claude-sonnet-4-20250514",
    "claude-haiku-3-5-20241022",
    "claude-opus-4-20250514",
];

// ---------------------------------------------------------------------------
// Anthropic API types (request)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct AnthropicRequest {
    model: String,
    messages: Vec<AnthropicMessage>,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stop_sequences: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<serde_json::Value>,
    stream: bool,
}

#[derive(Debug, Serialize)]
struct AnthropicMessage {
    role: String,
    content: Vec<AnthropicContent>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum AnthropicContent {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image {
        source: AnthropicImageSource,
    },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
    },
}

#[derive(Debug, Serialize)]
struct AnthropicImageSource {
    #[serde(rename = "type")]
    source_type: String,
    media_type: String,
    data: String,
}

// ---------------------------------------------------------------------------
// Anthropic API types (response)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct AnthropicResponse {
    id: String,
    model: String,
    content: Vec<AnthropicResponseContent>,
    stop_reason: Option<String>,
    usage: AnthropicUsage,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum AnthropicResponseContent {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
}

#[derive(Debug, Deserialize)]
struct AnthropicUsage {
    input_tokens: u32,
    output_tokens: u32,
}

// ---------------------------------------------------------------------------
// Anthropic SSE event types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum AnthropicStreamEvent {
    #[serde(rename = "message_start")]
    MessageStart { message: AnthropicStreamMessage },
    #[serde(rename = "content_block_start")]
    ContentBlockStart {
        index: u32,
        content_block: AnthropicStreamContentBlock,
    },
    #[serde(rename = "content_block_delta")]
    ContentBlockDelta {
        index: u32,
        delta: AnthropicStreamDelta,
    },
    #[serde(rename = "content_block_stop")]
    ContentBlockStop { index: u32 },
    #[serde(rename = "message_delta")]
    MessageDelta {
        delta: AnthropicMessageDelta,
        usage: AnthropicDeltaUsage,
    },
    #[serde(rename = "message_stop")]
    MessageStop,
    #[serde(rename = "ping")]
    Ping,
    #[serde(rename = "error")]
    Error { error: AnthropicStreamError },
}

#[derive(Debug, Deserialize)]
struct AnthropicStreamMessage {
    id: String,
    model: String,
    usage: AnthropicUsage,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum AnthropicStreamContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse { id: String, name: String },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum AnthropicStreamDelta {
    #[serde(rename = "text_delta")]
    TextDelta { text: String },
    #[serde(rename = "input_json_delta")]
    InputJsonDelta { partial_json: String },
}

#[derive(Debug, Deserialize)]
struct AnthropicMessageDelta {
    stop_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnthropicDeltaUsage {
    output_tokens: u32,
}

#[derive(Debug, Deserialize)]
struct AnthropicStreamError {
    message: String,
}

// ---------------------------------------------------------------------------
// Claude Provider
// ---------------------------------------------------------------------------

/// LLM provider that communicates with the Anthropic Messages API.
pub struct ClaudeProvider<T: TokenStorage> {
    http: Client,
    tokens: std::sync::Arc<T>,
}

impl<T: TokenStorage + 'static> ClaudeProvider<T> {
    /// Create a new Claude provider backed by the given token storage.
    pub fn new(tokens: std::sync::Arc<T>) -> Self {
        Self {
            http: Client::new(),
            tokens,
        }
    }

    /// Retrieve an access token or return an error.
    async fn get_token(&self) -> Result<String, ProviderError> {
        self.tokens
            .get_access_token("claude")
            .await?
            .ok_or_else(|| ProviderError::NoToken("claude".into()))
    }

    // -- conversion helpers --------------------------------------------------

    fn convert_request(request: &ChatRequest) -> AnthropicRequest {
        let mut system: Option<String> = None;
        let mut messages: Vec<AnthropicMessage> = Vec::new();

        for msg in &request.messages {
            match msg.role {
                MessageRole::System => {
                    if let Some(content) = &msg.content {
                        system = Some(content.as_text().to_string());
                    }
                }
                MessageRole::Tool => {
                    // Tool result messages map to Anthropic tool_result content
                    // within the previous assistant turn's response. We append
                    // as a user message with tool_result content.
                    let content_text = msg
                        .content
                        .as_ref()
                        .map(|c| c.as_text().to_string())
                        .unwrap_or_default();
                    let tool_use_id = msg.tool_call_id.clone().unwrap_or_default();
                    messages.push(AnthropicMessage {
                        role: "user".into(),
                        content: vec![AnthropicContent::ToolResult {
                            tool_use_id,
                            content: content_text,
                        }],
                    });
                }
                _ => {
                    let role = match msg.role {
                        MessageRole::User => "user",
                        MessageRole::Assistant => "assistant",
                        _ => "user",
                    };

                    let content = Self::convert_message_content(msg);
                    messages.push(AnthropicMessage {
                        role: role.into(),
                        content,
                    });
                }
            }
        }

        let stop_sequences = request.stop.as_ref().map(|s| match s {
            StopSequence::Single(s) => vec![s.clone()],
            StopSequence::Multiple(v) => v.clone(),
        });

        // Convert OpenAI tools to Anthropic tool format.
        let tools = request.tools.as_ref().map(|openai_tools| {
            openai_tools
                .iter()
                .map(|t| {
                    let name = t.function.name.clone();
                    let description = t.function.description.clone();
                    let input_schema = t
                        .function
                        .parameters
                        .clone()
                        .unwrap_or(serde_json::json!({"type": "object", "properties": {}}));
                    let mut tool = serde_json::json!({
                        "name": name,
                        "input_schema": input_schema,
                    });
                    if let Some(desc) = description {
                        tool["description"] = serde_json::Value::String(desc);
                    }
                    tool
                })
                .collect()
        });

        AnthropicRequest {
            model: request.model.clone(),
            messages,
            max_tokens: request.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
            system,
            temperature: request.temperature,
            top_p: request.top_p,
            stop_sequences,
            tools,
            tool_choice: request.tool_choice.clone(),
            stream: false,
        }
    }

    fn convert_message_content(msg: &ChatMessage) -> Vec<AnthropicContent> {
        let Some(content) = &msg.content else {
            return vec![];
        };

        match content {
            MessageContent::Text(text) => {
                let mut parts = vec![AnthropicContent::Text { text: text.clone() }];
                // Also append any tool_calls the assistant made.
                if let Some(tool_calls) = &msg.tool_calls {
                    for tc in tool_calls {
                        let input: serde_json::Value =
                            serde_json::from_str(&tc.function.arguments).unwrap_or_default();
                        parts.push(AnthropicContent::ToolUse {
                            id: tc.id.clone(),
                            name: tc.function.name.clone(),
                            input,
                        });
                    }
                }
                parts
            }
            MessageContent::Parts(parts) => parts
                .iter()
                .map(|p| match p {
                    ContentPart::Text { text } => AnthropicContent::Text { text: text.clone() },
                    ContentPart::ImageUrl { image_url } => {
                        // Anthropic expects base64-encoded image data, but
                        // we pass the URL through as-is and let the API
                        // decide. For true base64 data URIs, strip the prefix.
                        if image_url.url.starts_with("data:") {
                            let parts: Vec<&str> = image_url.url.splitn(2, ',').collect();
                            let media_type = parts
                                .first()
                                .unwrap_or(&"")
                                .trim_start_matches("data:")
                                .split(';')
                                .next()
                                .unwrap_or("image/png")
                                .to_string();
                            let data = parts.get(1).unwrap_or(&"").to_string();
                            AnthropicContent::Image {
                                source: AnthropicImageSource {
                                    source_type: "base64".into(),
                                    media_type,
                                    data,
                                },
                            }
                        } else {
                            AnthropicContent::Image {
                                source: AnthropicImageSource {
                                    source_type: "url".into(),
                                    media_type: "image/png".into(),
                                    data: image_url.url.clone(),
                                },
                            }
                        }
                    }
                })
                .collect(),
        }
    }

    fn convert_response(api_resp: AnthropicResponse) -> ChatResponse {
        let mut text_parts = Vec::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();

        for block in &api_resp.content {
            match block {
                AnthropicResponseContent::Text { text } => text_parts.push(text.clone()),
                AnthropicResponseContent::ToolUse { id, name, input } => {
                    tool_calls.push(ToolCall {
                        index: None,
                        id: id.clone(),
                        r#type: "function".into(),
                        function: FunctionCall {
                            name: name.clone(),
                            arguments: serde_json::to_string(input).unwrap_or_default(),
                        },
                    });
                }
            }
        }

        let content = text_parts.join("");

        let finish_reason = api_resp.stop_reason.map(|r| match r.as_str() {
            "end_turn" => "stop".into(),
            "max_tokens" => "length".into(),
            "tool_use" => "tool_calls".into(),
            other => other.to_string(),
        });

        ChatResponse {
            id: api_resp.id,
            object: "chat.completion".into(),
            created: chrono::Utc::now().timestamp(),
            model: api_resp.model,
            choices: vec![Choice {
                index: 0,
                message: ResponseMessage {
                    role: "assistant".into(),
                    content: if content.is_empty() {
                        None
                    } else {
                        Some(content)
                    },
                    tool_calls: if tool_calls.is_empty() {
                        None
                    } else {
                        Some(tool_calls)
                    },
                },
                finish_reason,
            }],
            usage: Usage {
                prompt_tokens: api_resp.usage.input_tokens,
                completion_tokens: api_resp.usage.output_tokens,
                total_tokens: api_resp.usage.input_tokens + api_resp.usage.output_tokens,
            },
        }
    }
}

// ---------------------------------------------------------------------------
// LlmProvider implementation
// ---------------------------------------------------------------------------

impl<T: TokenStorage + 'static> LlmProvider for ClaudeProvider<T> {
    fn id(&self) -> &str {
        "claude"
    }

    fn name(&self) -> &str {
        "Claude (Anthropic)"
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
                .post(format!("{API_BASE}/messages"))
                .header("x-api-key", &token)
                .header("anthropic-version", ANTHROPIC_VERSION)
                .header("content-type", "application/json")
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

            let api_resp: AnthropicResponse = resp.json().await?;
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
            .post(format!("{API_BASE}/messages"))
            .header("x-api-key", &token)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
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

        // We need to parse SSE lines from the byte stream.
        let model = request.model.clone();

        let sse_stream = byte_stream
            .map_err(|e| ProviderError::Stream(e.to_string()))
            .map(move |chunk_result| {
                let model = model.clone();
                (chunk_result, model)
            });

        // Buffer partial SSE lines and emit parsed events.
        let event_stream = stream::unfold(
            (Box::pin(sse_stream), String::new(), String::new(), 0u32, String::new()),
            |(mut inner_stream, mut buf, mut msg_id, mut input_tokens, mut model_name)| async move {
                loop {
                    // Try to parse complete lines from the buffer.
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

                            let event: AnthropicStreamEvent = match serde_json::from_str(data) {
                                Ok(e) => e,
                                Err(e) => {
                                    debug!(raw = %data, error = %e, "Failed to parse SSE event");
                                    continue;
                                }
                            };

                            match event {
                                AnthropicStreamEvent::MessageStart { message } => {
                                    msg_id = message.id.clone();
                                    input_tokens = message.usage.input_tokens;
                                    model_name = message.model.clone();
                                }
                                AnthropicStreamEvent::ContentBlockDelta { delta, .. } => {
                                    let text = match delta {
                                        AnthropicStreamDelta::TextDelta { text } => text,
                                        AnthropicStreamDelta::InputJsonDelta { partial_json } => {
                                            partial_json
                                        }
                                    };
                                    if !text.is_empty() {
                                        let chunk = ChatChunk {
                                            id: msg_id.clone(),
                                            object: "chat.completion.chunk".into(),
                                            created: chrono::Utc::now().timestamp(),
                                            model: model_name.clone(),
                                            choices: vec![ChunkChoice {
                                                index: 0,
                                                delta: Delta {
                                                    role: None,
                                                    content: Some(text),
                                                    tool_calls: None,
                                                },
                                                finish_reason: None,
                                            }],
                                            usage: None,
                                        };
                                        return Some((
                                            Ok(chunk),
                                            (inner_stream, buf, msg_id, input_tokens, model_name),
                                        ));
                                    }
                                }
                                AnthropicStreamEvent::MessageDelta { delta, usage } => {
                                    let finish_reason =
                                        delta.stop_reason.map(|r| match r.as_str() {
                                            "end_turn" => "stop".into(),
                                            "max_tokens" => "length".into(),
                                            "tool_use" => "tool_calls".into(),
                                            other => other.to_string(),
                                        });
                                    let chunk = ChatChunk {
                                        id: msg_id.clone(),
                                        object: "chat.completion.chunk".into(),
                                        created: chrono::Utc::now().timestamp(),
                                        model: model_name.clone(),
                                        choices: vec![ChunkChoice {
                                            index: 0,
                                            delta: Delta {
                                                role: None,
                                                content: None,
                                                tool_calls: None,
                                            },
                                            finish_reason,
                                        }],
                                        usage: Some(Usage {
                                            prompt_tokens: input_tokens,
                                            completion_tokens: usage.output_tokens,
                                            total_tokens: input_tokens + usage.output_tokens,
                                        }),
                                    };
                                    return Some((
                                        Ok(chunk),
                                        (inner_stream, buf, msg_id, input_tokens, model_name),
                                    ));
                                }
                                AnthropicStreamEvent::MessageStop => {
                                    return None;
                                }
                                AnthropicStreamEvent::Error { error } => {
                                    return Some((
                                        Err(ProviderError::Stream(error.message)),
                                        (inner_stream, buf, msg_id, input_tokens, model_name),
                                    ));
                                }
                                _ => {
                                    // Ping, ContentBlockStart, ContentBlockStop -- skip.
                                }
                            }
                        }
                        // Lines that don't start with "data:" (e.g. "event:") are ignored.
                    }

                    // Need more data from the network.
                    match inner_stream.next().await {
                        Some((Ok(bytes), model)) => {
                            if model_name.is_empty() {
                                model_name = model;
                            }
                            let text = String::from_utf8_lossy(&bytes);
                            buf.push_str(&text);
                        }
                        Some((Err(e), _)) => {
                            return Some((
                                Err(e),
                                (inner_stream, buf, msg_id, input_tokens, model_name),
                            ));
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
            .filter(|p| p.provider == "claude")
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

    /// Minimal in-memory token storage for testing.
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
        let p = ClaudeProvider::new(Arc::new(MockTokenStorage::empty()));
        assert_eq!(p.id(), "claude");
        assert_eq!(p.name(), "Claude (Anthropic)");
    }

    #[test]
    fn test_models_list() {
        let p = ClaudeProvider::new(Arc::new(MockTokenStorage::empty()));
        let models = p.models();
        assert!(models.contains(&"claude-sonnet-4-20250514".to_string()));
        assert!(models.contains(&"claude-haiku-3-5-20241022".to_string()));
        assert!(models.contains(&"claude-opus-4-20250514".to_string()));
    }

    #[test]
    fn test_supports_model() {
        let p = ClaudeProvider::new(Arc::new(MockTokenStorage::empty()));
        assert!(p.supports_model("claude-sonnet-4-20250514"));
        assert!(!p.supports_model("gpt-4o"));
    }

    #[tokio::test]
    async fn test_health_check_no_token() {
        let p = ClaudeProvider::new(Arc::new(MockTokenStorage::empty()));
        assert!(!p.health_check().await);
    }

    #[tokio::test]
    async fn test_health_check_with_token() {
        let p = ClaudeProvider::new(Arc::new(MockTokenStorage::with_token("test-token")));
        assert!(p.health_check().await);
    }

    #[tokio::test]
    async fn test_chat_no_token() {
        let p = ClaudeProvider::new(Arc::new(MockTokenStorage::empty()));
        let req = ChatRequest {
            model: "claude-sonnet-4-20250514".into(),
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
    fn test_convert_request_extracts_system() {
        let req = ChatRequest {
            model: "claude-sonnet-4-20250514".into(),
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
            temperature: Some(0.7),
            max_tokens: Some(1024),
            stream: false,
            top_p: None,
            stop: None,
            tools: None,
            tool_choice: None,
        };

        let converted = ClaudeProvider::<MockTokenStorage>::convert_request(&req);
        assert_eq!(converted.system, Some("You are helpful.".into()));
        assert_eq!(converted.messages.len(), 1); // system msg stripped
        assert_eq!(converted.messages[0].role, "user");
        assert_eq!(converted.max_tokens, 1024);
        assert_eq!(converted.temperature, Some(0.7));
    }

    #[test]
    fn test_convert_response() {
        let api_resp = AnthropicResponse {
            id: "msg_123".into(),
            model: "claude-sonnet-4-20250514".into(),
            content: vec![AnthropicResponseContent::Text {
                text: "Hello!".into(),
            }],
            stop_reason: Some("end_turn".into()),
            usage: AnthropicUsage {
                input_tokens: 10,
                output_tokens: 5,
            },
        };

        let resp = ClaudeProvider::<MockTokenStorage>::convert_response(api_resp);
        assert_eq!(resp.id, "msg_123");
        assert_eq!(resp.model, "claude-sonnet-4-20250514");
        assert_eq!(resp.choices.len(), 1);
        assert_eq!(
            resp.choices[0].message.content,
            Some("Hello!".to_string())
        );
        assert_eq!(
            resp.choices[0].finish_reason,
            Some("stop".to_string())
        );
        assert_eq!(resp.usage.prompt_tokens, 10);
        assert_eq!(resp.usage.completion_tokens, 5);
        assert_eq!(resp.usage.total_tokens, 15);
    }

    #[test]
    fn test_convert_response_tool_use() {
        let api_resp = AnthropicResponse {
            id: "msg_456".into(),
            model: "claude-sonnet-4-20250514".into(),
            content: vec![AnthropicResponseContent::ToolUse {
                id: "tool_1".into(),
                name: "get_weather".into(),
                input: serde_json::json!({"city": "London"}),
            }],
            stop_reason: Some("tool_use".into()),
            usage: AnthropicUsage {
                input_tokens: 20,
                output_tokens: 10,
            },
        };

        let resp = ClaudeProvider::<MockTokenStorage>::convert_response(api_resp);
        assert_eq!(
            resp.choices[0].finish_reason,
            Some("tool_calls".to_string())
        );
        let tc = resp.choices[0].message.tool_calls.as_ref().unwrap();
        assert_eq!(tc.len(), 1);
        assert_eq!(tc[0].function.name, "get_weather");
    }

    #[test]
    fn test_pricing_returns_claude_models() {
        let p = ClaudeProvider::new(Arc::new(MockTokenStorage::empty()));
        let pricing = p.pricing();
        assert!(!pricing.is_empty());
        for mp in &pricing {
            assert_eq!(mp.provider, "claude");
        }
    }

    #[test]
    fn test_convert_stop_sequences() {
        let req = ChatRequest {
            model: "claude-sonnet-4-20250514".into(),
            messages: vec![],
            temperature: None,
            max_tokens: None,
            stream: false,
            top_p: None,
            stop: Some(StopSequence::Multiple(vec![
                "END".into(),
                "STOP".into(),
            ])),
            tools: None,
            tool_choice: None,
        };

        let converted = ClaudeProvider::<MockTokenStorage>::convert_request(&req);
        assert_eq!(
            converted.stop_sequences,
            Some(vec!["END".to_string(), "STOP".to_string()])
        );
    }
}
