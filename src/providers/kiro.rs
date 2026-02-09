//! Kiro Provider (Amazon Q / AWS CodeWhisperer)
//!
//! Routes requests through the kiro-gateway client, which communicates with the
//! Kiro API using the Anthropic Messages API format internally. This provider
//! converts between the gaud OpenAI-compatible format and the Anthropic Messages
//! format that kiro-gateway expects.

use std::pin::Pin;
use std::sync::Arc;

use futures::stream::StreamExt;
use futures::Stream;
use tracing::debug;

use crate::providers::types::*;
use crate::providers::{LlmProvider, ProviderError};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Models exposed via this provider.
///
/// Users request these model names and they are resolved internally by the
/// kiro-gateway model resolver to Kiro API model identifiers.
const SUPPORTED_MODELS: &[&str] = &[
    "kiro:auto",
    "kiro:claude-sonnet-4",
    "kiro:claude-sonnet-4.5",
    "kiro:claude-haiku-4.5",
    "kiro:claude-opus-4.5",
    "kiro:claude-3.7-sonnet",
];

/// Default max tokens when the request doesn't specify one.
const DEFAULT_MAX_TOKENS: u32 = 8192;

// ---------------------------------------------------------------------------
// KiroProvider
// ---------------------------------------------------------------------------

/// LLM provider that communicates through the Kiro gateway client.
///
/// The `KiroClient` handles authentication (refresh tokens, AWS SSO OIDC)
/// and API communication internally. This provider just wraps it with format
/// conversion between OpenAI types and Anthropic Messages types.
pub struct KiroProvider {
    client: Arc<kiro_gateway::KiroClient>,
}

impl KiroProvider {
    /// Create a new Kiro provider wrapping an already-built KiroClient.
    pub fn new(client: kiro_gateway::KiroClient) -> Self {
        Self {
            client: Arc::new(client),
        }
    }

    /// Strip the `kiro:` prefix from a model name for the kiro-gateway client.
    fn strip_prefix(model: &str) -> &str {
        model.strip_prefix("kiro:").unwrap_or(model)
    }

    // -- OpenAI -> Anthropic Messages conversion ----------------------------

    fn convert_request(request: &ChatRequest) -> kiro_gateway::MessagesRequest {
        let model = Self::strip_prefix(&request.model).to_string();
        let max_tokens = request.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS);

        let mut system: Option<kiro_gateway::SystemPrompt> = None;
        let mut messages: Vec<kiro_gateway::Message> = Vec::new();

        for msg in &request.messages {
            match msg.role {
                MessageRole::System => {
                    // Collect system messages into the system prompt.
                    if let Some(ref content) = msg.content {
                        let text = content.as_text().to_string();
                        system = match system.take() {
                            Some(kiro_gateway::SystemPrompt::Text(existing)) => {
                                Some(kiro_gateway::SystemPrompt::Text(format!(
                                    "{}\n{}",
                                    existing, text
                                )))
                            }
                            _ => Some(kiro_gateway::SystemPrompt::Text(text)),
                        };
                    }
                }
                MessageRole::User => {
                    let content = Self::convert_content_to_kiro(msg);
                    messages.push(kiro_gateway::Message {
                        role: kiro_gateway::Role::User,
                        content,
                    });
                }
                MessageRole::Assistant => {
                    let content = Self::convert_assistant_to_kiro(msg);
                    messages.push(kiro_gateway::Message {
                        role: kiro_gateway::Role::Assistant,
                        content,
                    });
                }
                MessageRole::Tool => {
                    // Tool results are sent as user messages with tool_result content blocks.
                    let tool_call_id = msg.tool_call_id.clone().unwrap_or_default();
                    let text = msg
                        .content
                        .as_ref()
                        .map(|c| c.as_text().to_string())
                        .unwrap_or_default();
                    // OpenAI tool messages don't carry an explicit is_error
                    // field. We infer error status from the content: if it
                    // starts with "Error:" or contains a recognizable error
                    // pattern, mark it as an error for the Kiro API.
                    let is_error = text.starts_with("Error:")
                        || text.starts_with("error:")
                        || text.starts_with("ERROR:");
                    messages.push(kiro_gateway::Message {
                        role: kiro_gateway::Role::User,
                        content: kiro_gateway::MessageContent::Blocks(vec![
                            kiro_gateway::ContentBlock::ToolResult {
                                tool_use_id: tool_call_id,
                                content: kiro_gateway::models::request::ToolResultContent::Text(
                                    text,
                                ),
                                is_error,
                            },
                        ]),
                    });
                }
            }
        }

        // Convert stop sequences.
        let stop_sequences = request.stop.as_ref().map(|s| match s {
            StopSequence::Single(s) => vec![s.clone()],
            StopSequence::Multiple(v) => v.clone(),
        });

        // Convert tools.
        let tools = request.tools.as_ref().map(|ts| {
            ts.iter()
                .map(|t| kiro_gateway::Tool {
                    name: t.function.name.clone(),
                    description: t.function.description.clone(),
                    input_schema: t
                        .function
                        .parameters
                        .clone()
                        .unwrap_or(serde_json::json!({"type": "object"})),
                })
                .collect()
        });

        // Convert tool_choice.
        let tool_choice = request.tool_choice.as_ref().and_then(|tc| {
            if let Some(s) = tc.as_str() {
                match s {
                    "auto" => Some(kiro_gateway::ToolChoice::Auto),
                    "none" => Some(kiro_gateway::ToolChoice::None),
                    "any" | "required" => Some(kiro_gateway::ToolChoice::Any),
                    _ => None,
                }
            } else if let Some(obj) = tc.as_object() {
                // {"type": "function", "function": {"name": "foo"}}
                obj.get("function")
                    .and_then(|f| f.get("name"))
                    .and_then(|n| n.as_str())
                    .map(|name| kiro_gateway::ToolChoice::Tool {
                        name: name.to_string(),
                    })
            } else {
                None
            }
        });

        kiro_gateway::MessagesRequest {
            model,
            max_tokens,
            messages,
            system,
            tools,
            tool_choice,
            stream: false,
            temperature: request.temperature,
            top_p: request.top_p,
            stop_sequences,
            thinking: None,
        }
    }

    fn convert_content_to_kiro(msg: &ChatMessage) -> kiro_gateway::MessageContent {
        match &msg.content {
            Some(MessageContent::Text(t)) => kiro_gateway::MessageContent::Text(t.clone()),
            Some(MessageContent::Parts(parts)) => {
                let blocks: Vec<kiro_gateway::ContentBlock> = parts
                    .iter()
                    .map(|p| match p {
                        ContentPart::Text { text } => {
                            kiro_gateway::ContentBlock::Text { text: text.clone() }
                        }
                        ContentPart::ImageUrl { image_url } => {
                            // Attempt to parse data URIs; otherwise pass as-is.
                            if let Some((media_type, data)) =
                                parse_data_uri(&image_url.url)
                            {
                                kiro_gateway::ContentBlock::Image {
                                    source: kiro_gateway::models::request::ImageSource {
                                        source_type: "base64".to_string(),
                                        media_type,
                                        data,
                                    },
                                }
                            } else {
                                // URLs can't be passed directly to Anthropic Messages API,
                                // so treat as text fallback.
                                kiro_gateway::ContentBlock::Text {
                                    text: format!("[Image: {}]", image_url.url),
                                }
                            }
                        }
                    })
                    .collect();
                kiro_gateway::MessageContent::Blocks(blocks)
            }
            None => kiro_gateway::MessageContent::Text(String::new()),
        }
    }

    fn convert_assistant_to_kiro(msg: &ChatMessage) -> kiro_gateway::MessageContent {
        let mut blocks: Vec<kiro_gateway::ContentBlock> = Vec::new();

        // Add text content if present.
        if let Some(ref content) = msg.content {
            let text = content.as_text();
            if !text.is_empty() {
                blocks.push(kiro_gateway::ContentBlock::Text {
                    text: text.to_string(),
                });
            }
        }

        // Add tool calls if present.
        if let Some(ref tool_calls) = msg.tool_calls {
            for tc in tool_calls {
                let input: serde_json::Value =
                    serde_json::from_str(&tc.function.arguments).unwrap_or(serde_json::json!({}));
                blocks.push(kiro_gateway::ContentBlock::ToolUse {
                    id: tc.id.clone(),
                    name: tc.function.name.clone(),
                    input,
                });
            }
        }

        if blocks.is_empty() {
            kiro_gateway::MessageContent::Text(String::new())
        } else if blocks.len() == 1 {
            if let kiro_gateway::ContentBlock::Text { text } = &blocks[0] {
                return kiro_gateway::MessageContent::Text(text.clone());
            }
            kiro_gateway::MessageContent::Blocks(blocks)
        } else {
            kiro_gateway::MessageContent::Blocks(blocks)
        }
    }

    // -- Anthropic Messages -> OpenAI response conversion -------------------

    fn convert_response(
        resp: kiro_gateway::MessagesResponse,
        original_model: &str,
    ) -> ChatResponse {
        let mut content_text: Option<String> = None;
        let mut tool_calls: Vec<ToolCall> = Vec::new();

        for block in &resp.content {
            match block {
                kiro_gateway::ResponseContentBlock::Text { text } => {
                    content_text.get_or_insert_with(String::new).push_str(text);
                }
                kiro_gateway::ResponseContentBlock::ToolUse { id, name, input } => {
                    tool_calls.push(ToolCall {
                        index: None,
                        id: id.clone(),
                        r#type: "function".to_string(),
                        function: FunctionCall {
                            name: name.clone(),
                            arguments: serde_json::to_string(input).unwrap_or_default(),
                        },
                    });
                }
                kiro_gateway::ResponseContentBlock::Thinking { .. } => {
                    // Thinking blocks are internal; don't expose in OpenAI format.
                }
            }
        }

        let finish_reason = resp.stop_reason.map(|sr| {
            match sr {
                kiro_gateway::StopReason::EndTurn => "stop",
                kiro_gateway::StopReason::ToolUse => "tool_calls",
                kiro_gateway::StopReason::MaxTokens => "length",
                kiro_gateway::StopReason::StopSequence => "stop",
            }
            .to_string()
        });

        ChatResponse {
            id: resp.id.clone(),
            object: "chat.completion".to_string(),
            created: chrono::Utc::now().timestamp(),
            model: original_model.to_string(),
            choices: vec![Choice {
                index: 0,
                message: ResponseMessage {
                    role: "assistant".to_string(),
                    content: content_text,
                    tool_calls: if tool_calls.is_empty() {
                        None
                    } else {
                        Some(tool_calls)
                    },
                },
                finish_reason,
            }],
            usage: Usage {
                prompt_tokens: resp.usage.input_tokens,
                completion_tokens: resp.usage.output_tokens,
                total_tokens: resp.usage.input_tokens + resp.usage.output_tokens,
            },
        }
    }

    fn convert_stream_event(
        event: kiro_gateway::StreamEvent,
        model: &str,
        response_id: &str,
        stream_state: &mut StreamState,
    ) -> Option<ChatChunk> {
        match event {
            kiro_gateway::StreamEvent::MessageStart { .. } => {
                // Emit the initial chunk with role.
                Some(ChatChunk {
                    id: response_id.to_string(),
                    object: "chat.completion.chunk".to_string(),
                    created: chrono::Utc::now().timestamp(),
                    model: model.to_string(),
                    choices: vec![ChunkChoice {
                        index: 0,
                        delta: Delta {
                            role: Some("assistant".to_string()),
                            content: None,
                            tool_calls: None,
                        },
                        finish_reason: None,
                    }],
                    usage: None,
                })
            }
            kiro_gateway::StreamEvent::ContentBlockStart {
                content_block, ..
            } => {
                // When a tool_use block starts, emit the initial tool_calls chunk
                // with the tool name and id (OpenAI streaming format).
                if let kiro_gateway::ResponseContentBlock::ToolUse { id, name, .. } =
                    content_block
                {
                    let idx = stream_state.tool_call_index;
                    stream_state.tool_call_index += 1;
                    stream_state.current_tool_index = Some(idx);

                    return Some(ChatChunk {
                        id: response_id.to_string(),
                        object: "chat.completion.chunk".to_string(),
                        created: chrono::Utc::now().timestamp(),
                        model: model.to_string(),
                        choices: vec![ChunkChoice {
                            index: 0,
                            delta: Delta {
                                role: None,
                                content: None,
                                tool_calls: Some(vec![ToolCall {
                                    index: Some(idx),
                                    id,
                                    r#type: "function".to_string(),
                                    function: FunctionCall {
                                        name,
                                        arguments: String::new(),
                                    },
                                }]),
                            },
                            finish_reason: None,
                        }],
                        usage: None,
                    });
                }
                None
            }
            kiro_gateway::StreamEvent::ContentBlockDelta { delta, .. } => {
                match delta {
                    kiro_gateway::ContentDelta::TextDelta { text } => Some(ChatChunk {
                        id: response_id.to_string(),
                        object: "chat.completion.chunk".to_string(),
                        created: chrono::Utc::now().timestamp(),
                        model: model.to_string(),
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
                    }),
                    kiro_gateway::ContentDelta::InputJsonDelta { partial_json } => {
                        // Emit as a tool_calls argument delta (OpenAI streaming format).
                        let tool_idx =
                            stream_state.current_tool_index.unwrap_or(0);

                        Some(ChatChunk {
                            id: response_id.to_string(),
                            object: "chat.completion.chunk".to_string(),
                            created: chrono::Utc::now().timestamp(),
                            model: model.to_string(),
                            choices: vec![ChunkChoice {
                                index: 0,
                                delta: Delta {
                                    role: None,
                                    content: None,
                                    tool_calls: Some(vec![ToolCall {
                                        index: Some(tool_idx),
                                        id: String::new(),
                                        r#type: "function".to_string(),
                                        function: FunctionCall {
                                            name: String::new(),
                                            arguments: partial_json,
                                        },
                                    }]),
                                },
                                finish_reason: None,
                            }],
                            usage: None,
                        })
                    }
                    kiro_gateway::ContentDelta::ThinkingDelta { .. } => None,
                }
            }
            kiro_gateway::StreamEvent::MessageDelta { delta, usage } => {
                let finish_reason = delta.stop_reason.map(|sr| {
                    match sr {
                        kiro_gateway::StopReason::EndTurn => "stop",
                        kiro_gateway::StopReason::ToolUse => "tool_calls",
                        kiro_gateway::StopReason::MaxTokens => "length",
                        kiro_gateway::StopReason::StopSequence => "stop",
                    }
                    .to_string()
                });

                let usage = usage.map(|u| Usage {
                    prompt_tokens: u.input_tokens,
                    completion_tokens: u.output_tokens,
                    total_tokens: u.input_tokens + u.output_tokens,
                });

                Some(ChatChunk {
                    id: response_id.to_string(),
                    object: "chat.completion.chunk".to_string(),
                    created: chrono::Utc::now().timestamp(),
                    model: model.to_string(),
                    choices: vec![ChunkChoice {
                        index: 0,
                        delta: Delta {
                            role: None,
                            content: None,
                            tool_calls: None,
                        },
                        finish_reason,
                    }],
                    usage,
                })
            }
            kiro_gateway::StreamEvent::MessageStop => None,
            kiro_gateway::StreamEvent::Ping => None,
            kiro_gateway::StreamEvent::ContentBlockStop { .. } => {
                stream_state.current_tool_index = None;
                None
            }
            kiro_gateway::StreamEvent::Error { error } => {
                tracing::warn!(
                    error_type = %error.error_type,
                    error_message = %error.message,
                    "Kiro stream error event"
                );
                // Propagate as a final chunk with error info so the client
                // learns the stream had an error, rather than silently dropping.
                Some(ChatChunk {
                    id: response_id.to_string(),
                    object: "chat.completion.chunk".to_string(),
                    created: chrono::Utc::now().timestamp(),
                    model: model.to_string(),
                    choices: vec![ChunkChoice {
                        index: 0,
                        delta: Delta {
                            role: None,
                            content: Some(format!("[Stream error: {}]", error.message)),
                            tool_calls: None,
                        },
                        finish_reason: Some("error".to_string()),
                    }],
                    usage: None,
                })
            }
        }
    }
}

/// Mutable state tracked across streaming events for proper tool_calls handling.
#[derive(Default)]
struct StreamState {
    /// Next tool_call index to assign (OpenAI uses sequential indices).
    tool_call_index: u32,
    /// Index of the tool_call currently being streamed (for InputJsonDelta).
    current_tool_index: Option<u32>,
}

/// Parse a data URI (data:image/jpeg;base64,...) into (media_type, base64_data).
fn parse_data_uri(url: &str) -> Option<(String, String)> {
    let rest = url.strip_prefix("data:")?;
    let (header, data) = rest.split_once(',')?;
    if !header.ends_with(";base64") {
        return None;
    }
    let media_type = header.strip_suffix(";base64")?;
    Some((media_type.to_string(), data.to_string()))
}

/// Convert a kiro-gateway error into a ProviderError.
fn convert_kiro_error(err: kiro_gateway::Error) -> ProviderError {
    match err {
        kiro_gateway::Error::NotAuthenticated | kiro_gateway::Error::TokenExpired => {
            ProviderError::NoToken("kiro".into())
        }
        kiro_gateway::Error::RefreshFailed(msg) => ProviderError::NoToken(format!("kiro: {msg}")),
        kiro_gateway::Error::Api { status, message } => ProviderError::Api { status, message },
        kiro_gateway::Error::RateLimited { retry_after } => ProviderError::RateLimited {
            retry_after_secs: retry_after
                .map(|d| d.as_secs())
                .unwrap_or(60),
        },
        kiro_gateway::Error::Stream(msg) => ProviderError::Stream(msg),
        kiro_gateway::Error::Network(e) => ProviderError::Http(e),
        other => ProviderError::Other(other.to_string()),
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
        SUPPORTED_MODELS.iter().map(|s| s.to_string()).collect()
    }

    fn supports_model(&self, model: &str) -> bool {
        SUPPORTED_MODELS.iter().any(|m| *m == model)
    }

    fn chat(
        &self,
        request: &ChatRequest,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<ChatResponse, ProviderError>> + Send + '_>>
    {
        let request = request.clone();
        Box::pin(async move {
            let kiro_req = Self::convert_request(&request);
            let response = self
                .client
                .send_messages(kiro_req)
                .await
                .map_err(convert_kiro_error)?;
            Ok(Self::convert_response(response, &request.model))
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
            let mut kiro_req = Self::convert_request(&request);
            kiro_req.stream = true;

            let kiro_stream = self
                .client
                .send_messages_stream(kiro_req)
                .await
                .map_err(convert_kiro_error)?;

            let model = request.model.clone();
            let response_id = format!("chatcmpl-kiro-{}", uuid::Uuid::new_v4());
            let mut stream_state = StreamState::default();

            let event_stream = kiro_stream.filter_map(move |result| {
                let chunk = match result {
                    Ok(event) => Self::convert_stream_event(
                        event,
                        &model,
                        &response_id,
                        &mut stream_state,
                    )
                    .map(Ok),
                    Err(e) => Some(Err(convert_kiro_error(e))),
                };
                async move { chunk }
            });

            Ok(Box::pin(event_stream)
                as Pin<Box<dyn Stream<Item = Result<ChatChunk, ProviderError>> + Send>>)
        })
    }

    fn health_check(&self) -> Pin<Box<dyn std::future::Future<Output = bool> + Send + '_>> {
        Box::pin(async move {
            // Try to list models as a lightweight health check.
            self.client.list_models().await.is_ok()
        })
    }

    fn pricing(&self) -> Vec<ModelPricing> {
        crate::providers::cost::CostDatabase::all()
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
    fn test_strip_prefix() {
        assert_eq!(KiroProvider::strip_prefix("kiro:claude-sonnet-4"), "claude-sonnet-4");
        assert_eq!(KiroProvider::strip_prefix("claude-sonnet-4"), "claude-sonnet-4");
        assert_eq!(KiroProvider::strip_prefix("kiro:auto"), "auto");
    }

    #[test]
    fn test_supported_models() {
        // All models should have the kiro: prefix.
        for m in SUPPORTED_MODELS {
            assert!(m.starts_with("kiro:"), "Model {} should start with kiro:", m);
        }
    }

    #[test]
    fn test_parse_data_uri_valid() {
        let (mt, data) = parse_data_uri("data:image/jpeg;base64,/9j/4AAQ").unwrap();
        assert_eq!(mt, "image/jpeg");
        assert_eq!(data, "/9j/4AAQ");
    }

    #[test]
    fn test_parse_data_uri_png() {
        let (mt, data) = parse_data_uri("data:image/png;base64,iVBOR").unwrap();
        assert_eq!(mt, "image/png");
        assert_eq!(data, "iVBOR");
    }

    #[test]
    fn test_parse_data_uri_invalid() {
        assert!(parse_data_uri("https://example.com/image.png").is_none());
        assert!(parse_data_uri("data:image/jpeg,raw-data").is_none());
        assert!(parse_data_uri("not-a-uri").is_none());
    }

    #[test]
    fn test_convert_request_basic() {
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
        };

        let kiro_req = KiroProvider::convert_request(&req);
        assert_eq!(kiro_req.model, "claude-sonnet-4");
        assert_eq!(kiro_req.max_tokens, 4096);
        assert_eq!(kiro_req.temperature, Some(0.7));
        assert_eq!(kiro_req.messages.len(), 1); // Only user message; system is separate.
        assert!(kiro_req.system.is_some());

        if let Some(kiro_gateway::SystemPrompt::Text(sys)) = &kiro_req.system {
            assert_eq!(sys, "You are helpful.");
        } else {
            panic!("Expected text system prompt");
        }
    }

    #[test]
    fn test_convert_request_default_max_tokens() {
        let req = ChatRequest {
            model: "kiro:auto".into(),
            messages: vec![ChatMessage {
                role: MessageRole::User,
                content: Some(MessageContent::Text("Hi".into())),
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

        let kiro_req = KiroProvider::convert_request(&req);
        assert_eq!(kiro_req.max_tokens, DEFAULT_MAX_TOKENS);
    }

    #[test]
    fn test_convert_request_with_stop_sequences() {
        let req = ChatRequest {
            model: "kiro:auto".into(),
            messages: vec![],
            temperature: None,
            max_tokens: None,
            stream: false,
            top_p: None,
            stop: Some(StopSequence::Multiple(vec!["END".into(), "STOP".into()])),
            tools: None,
            tool_choice: None,
        };

        let kiro_req = KiroProvider::convert_request(&req);
        assert_eq!(
            kiro_req.stop_sequences,
            Some(vec!["END".to_string(), "STOP".to_string()])
        );
    }

    #[test]
    fn test_convert_request_with_tools() {
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
        };

        let kiro_req = KiroProvider::convert_request(&req);
        assert!(kiro_req.tools.is_some());
        let tools = kiro_req.tools.unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "search");
        assert_eq!(tools[0].description, Some("Search the web".to_string()));
    }

    #[test]
    fn test_convert_request_tool_message() {
        let req = ChatRequest {
            model: "kiro:auto".into(),
            messages: vec![ChatMessage {
                role: MessageRole::Tool,
                content: Some(MessageContent::Text("result data".into())),
                name: None,
                tool_calls: None,
                tool_call_id: Some("call_123".into()),
            }],
            temperature: None,
            max_tokens: None,
            stream: false,
            top_p: None,
            stop: None,
            tools: None,
            tool_choice: None,
        };

        let kiro_req = KiroProvider::convert_request(&req);
        assert_eq!(kiro_req.messages.len(), 1);
        assert_eq!(kiro_req.messages[0].role, kiro_gateway::Role::User);
    }

    #[test]
    fn test_convert_request_assistant_with_tool_calls() {
        let req = ChatRequest {
            model: "kiro:auto".into(),
            messages: vec![ChatMessage {
                role: MessageRole::Assistant,
                content: None,
                name: None,
                tool_calls: Some(vec![ToolCall {
                    index: None,
                    id: "call_1".into(),
                    r#type: "function".into(),
                    function: FunctionCall {
                        name: "search".into(),
                        arguments: r#"{"q":"test"}"#.into(),
                    },
                }]),
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

        let kiro_req = KiroProvider::convert_request(&req);
        assert_eq!(kiro_req.messages.len(), 1);
        assert_eq!(kiro_req.messages[0].role, kiro_gateway::Role::Assistant);
        match &kiro_req.messages[0].content {
            kiro_gateway::MessageContent::Blocks(blocks) => {
                assert_eq!(blocks.len(), 1);
                match &blocks[0] {
                    kiro_gateway::ContentBlock::ToolUse { id, name, input } => {
                        assert_eq!(id, "call_1");
                        assert_eq!(name, "search");
                        assert_eq!(input, &serde_json::json!({"q": "test"}));
                    }
                    _ => panic!("Expected ToolUse block"),
                }
            }
            _ => panic!("Expected Blocks content"),
        }
    }

    #[test]
    fn test_convert_response_text() {
        let resp = kiro_gateway::MessagesResponse {
            id: "msg_123".into(),
            response_type: "message".into(),
            role: "assistant".into(),
            content: vec![kiro_gateway::ResponseContentBlock::Text {
                text: "Hello there!".into(),
            }],
            model: "claude-sonnet-4".into(),
            stop_reason: Some(kiro_gateway::StopReason::EndTurn),
            stop_sequence: None,
            usage: kiro_gateway::Usage {
                input_tokens: 10,
                output_tokens: 5,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            },
        };

        let chat_resp = KiroProvider::convert_response(resp, "kiro:claude-sonnet-4");
        assert_eq!(chat_resp.id, "msg_123");
        assert_eq!(chat_resp.model, "kiro:claude-sonnet-4");
        assert_eq!(
            chat_resp.choices[0].message.content,
            Some("Hello there!".to_string())
        );
        assert_eq!(
            chat_resp.choices[0].finish_reason,
            Some("stop".to_string())
        );
        assert_eq!(chat_resp.usage.prompt_tokens, 10);
        assert_eq!(chat_resp.usage.completion_tokens, 5);
        assert_eq!(chat_resp.usage.total_tokens, 15);
    }

    #[test]
    fn test_convert_response_tool_use() {
        let resp = kiro_gateway::MessagesResponse {
            id: "msg_456".into(),
            response_type: "message".into(),
            role: "assistant".into(),
            content: vec![kiro_gateway::ResponseContentBlock::ToolUse {
                id: "toolu_1".into(),
                name: "search".into(),
                input: serde_json::json!({"q": "test"}),
            }],
            model: "claude-sonnet-4".into(),
            stop_reason: Some(kiro_gateway::StopReason::ToolUse),
            stop_sequence: None,
            usage: kiro_gateway::Usage::default(),
        };

        let chat_resp = KiroProvider::convert_response(resp, "kiro:auto");
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
    fn test_convert_response_max_tokens() {
        let resp = kiro_gateway::MessagesResponse {
            id: "msg_789".into(),
            response_type: "message".into(),
            role: "assistant".into(),
            content: vec![kiro_gateway::ResponseContentBlock::Text {
                text: "partial...".into(),
            }],
            model: "claude-sonnet-4".into(),
            stop_reason: Some(kiro_gateway::StopReason::MaxTokens),
            stop_sequence: None,
            usage: kiro_gateway::Usage::default(),
        };

        let chat_resp = KiroProvider::convert_response(resp, "kiro:auto");
        assert_eq!(
            chat_resp.choices[0].finish_reason,
            Some("length".to_string())
        );
    }

    #[test]
    fn test_convert_kiro_error_auth() {
        let err = convert_kiro_error(kiro_gateway::Error::NotAuthenticated);
        assert!(matches!(err, ProviderError::NoToken(_)));

        let err = convert_kiro_error(kiro_gateway::Error::TokenExpired);
        assert!(matches!(err, ProviderError::NoToken(_)));
    }

    #[test]
    fn test_convert_kiro_error_api() {
        let err = convert_kiro_error(kiro_gateway::Error::Api {
            status: 500,
            message: "Internal error".into(),
        });
        assert!(matches!(err, ProviderError::Api { status: 500, .. }));
    }

    #[test]
    fn test_convert_kiro_error_rate_limited() {
        let err = convert_kiro_error(kiro_gateway::Error::RateLimited {
            retry_after: Some(std::time::Duration::from_secs(30)),
        });
        match err {
            ProviderError::RateLimited { retry_after_secs } => {
                assert_eq!(retry_after_secs, 30);
            }
            _ => panic!("Expected RateLimited"),
        }
    }

    #[test]
    fn test_convert_request_multiple_system_messages() {
        let req = ChatRequest {
            model: "kiro:auto".into(),
            messages: vec![
                ChatMessage {
                    role: MessageRole::System,
                    content: Some(MessageContent::Text("First system.".into())),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
                ChatMessage {
                    role: MessageRole::System,
                    content: Some(MessageContent::Text("Second system.".into())),
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
            temperature: None,
            max_tokens: None,
            stream: false,
            top_p: None,
            stop: None,
            tools: None,
            tool_choice: None,
        };

        let kiro_req = KiroProvider::convert_request(&req);
        // Multiple system messages should be concatenated.
        if let Some(kiro_gateway::SystemPrompt::Text(sys)) = &kiro_req.system {
            assert!(sys.contains("First system."));
            assert!(sys.contains("Second system."));
        } else {
            panic!("Expected text system prompt");
        }
        // Only user message in messages list.
        assert_eq!(kiro_req.messages.len(), 1);
    }

    #[test]
    fn test_convert_request_multipart_content() {
        let req = ChatRequest {
            model: "kiro:auto".into(),
            messages: vec![ChatMessage {
                role: MessageRole::User,
                content: Some(MessageContent::Parts(vec![
                    ContentPart::Text {
                        text: "What is this?".into(),
                    },
                    ContentPart::ImageUrl {
                        image_url: ImageUrl {
                            url: "data:image/png;base64,iVBOR".into(),
                            detail: None,
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

        let kiro_req = KiroProvider::convert_request(&req);
        match &kiro_req.messages[0].content {
            kiro_gateway::MessageContent::Blocks(blocks) => {
                assert_eq!(blocks.len(), 2);
                match &blocks[0] {
                    kiro_gateway::ContentBlock::Text { text } => {
                        assert_eq!(text, "What is this?");
                    }
                    _ => panic!("Expected text block"),
                }
                match &blocks[1] {
                    kiro_gateway::ContentBlock::Image { source } => {
                        assert_eq!(source.media_type, "image/png");
                        assert_eq!(source.data, "iVBOR");
                    }
                    _ => panic!("Expected image block"),
                }
            }
            _ => panic!("Expected blocks content"),
        }
    }

    #[test]
    fn test_convert_stream_event_message_start() {
        let event = kiro_gateway::StreamEvent::MessageStart {
            message: kiro_gateway::models::stream::PartialMessage {
                id: "msg_1".into(),
                message_type: "message".into(),
                role: "assistant".into(),
                model: "claude-sonnet-4".into(),
                usage: kiro_gateway::Usage::default(),
            },
        };

        let mut state = StreamState::default();
        let chunk = KiroProvider::convert_stream_event(event, "kiro:auto", "resp-1", &mut state);
        assert!(chunk.is_some());
        let chunk = chunk.unwrap();
        assert_eq!(chunk.choices[0].delta.role, Some("assistant".to_string()));
    }

    #[test]
    fn test_convert_stream_event_text_delta() {
        let event = kiro_gateway::StreamEvent::ContentBlockDelta {
            index: 0,
            delta: kiro_gateway::ContentDelta::TextDelta {
                text: "Hello".into(),
            },
        };

        let mut state = StreamState::default();
        let chunk = KiroProvider::convert_stream_event(event, "kiro:auto", "resp-1", &mut state);
        assert!(chunk.is_some());
        let chunk = chunk.unwrap();
        assert_eq!(chunk.choices[0].delta.content, Some("Hello".to_string()));
    }

    #[test]
    fn test_convert_stream_event_message_delta() {
        let event = kiro_gateway::StreamEvent::MessageDelta {
            delta: kiro_gateway::models::stream::MessageDelta {
                stop_reason: Some(kiro_gateway::StopReason::EndTurn),
                stop_sequence: None,
            },
            usage: Some(kiro_gateway::Usage {
                input_tokens: 10,
                output_tokens: 20,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            }),
        };

        let mut state = StreamState::default();
        let chunk = KiroProvider::convert_stream_event(event, "kiro:auto", "resp-1", &mut state);
        assert!(chunk.is_some());
        let chunk = chunk.unwrap();
        assert_eq!(
            chunk.choices[0].finish_reason,
            Some("stop".to_string())
        );
        assert!(chunk.usage.is_some());
    }

    #[test]
    fn test_convert_stream_event_ping_returns_none() {
        let event = kiro_gateway::StreamEvent::Ping;
        let mut state = StreamState::default();
        assert!(KiroProvider::convert_stream_event(event, "kiro:auto", "resp-1", &mut state).is_none());
    }

    #[test]
    fn test_convert_stream_event_stop_returns_none() {
        let event = kiro_gateway::StreamEvent::MessageStop;
        let mut state = StreamState::default();
        assert!(KiroProvider::convert_stream_event(event, "kiro:auto", "resp-1", &mut state).is_none());
    }

    #[test]
    fn test_convert_stream_event_tool_call() {
        let mut state = StreamState::default();

        // ContentBlockStart with ToolUse emits initial tool_calls chunk
        let start_event = kiro_gateway::StreamEvent::ContentBlockStart {
            index: 0,
            content_block: kiro_gateway::ResponseContentBlock::ToolUse {
                id: "toolu_1".into(),
                name: "search".into(),
                input: serde_json::json!({}),
            },
        };
        let chunk = KiroProvider::convert_stream_event(start_event, "kiro:auto", "resp-1", &mut state);
        assert!(chunk.is_some());
        let chunk = chunk.unwrap();
        let tc = chunk.choices[0].delta.tool_calls.as_ref().unwrap();
        assert_eq!(tc[0].index, Some(0));
        assert_eq!(tc[0].id, "toolu_1");
        assert_eq!(tc[0].function.name, "search");
        assert_eq!(tc[0].function.arguments, "");

        // InputJsonDelta emits argument delta with correct index
        let delta_event = kiro_gateway::StreamEvent::ContentBlockDelta {
            index: 0,
            delta: kiro_gateway::ContentDelta::InputJsonDelta {
                partial_json: r#"{"q":"te"#.into(),
            },
        };
        let chunk = KiroProvider::convert_stream_event(delta_event, "kiro:auto", "resp-1", &mut state);
        assert!(chunk.is_some());
        let chunk = chunk.unwrap();
        // Should be in tool_calls, NOT in content
        assert!(chunk.choices[0].delta.content.is_none());
        let tc = chunk.choices[0].delta.tool_calls.as_ref().unwrap();
        assert_eq!(tc[0].index, Some(0));
        assert_eq!(tc[0].function.arguments, r#"{"q":"te"#);
    }
}
