//! Kiro Provider (Amazon Q / AWS CodeWhisperer)
//!
//! Routes requests through the kiro-gateway client, which communicates with the
//! Kiro API using the Anthropic Messages API format internally. This provider
//! converts between the gaud OpenAI-compatible format and the Anthropic Messages
//! format that kiro-gateway expects.
//!
//! Format conversion is delegated to [`KiroTransformer`] and [`KiroStreamState`]
//! (in `providers::transform::kiro`), which are well-tested independently.
//! This module only handles transport (via `kiro_gateway::KiroClient`) and
//! error mapping.

use std::pin::Pin;
use std::sync::Arc;

use futures::stream::StreamExt;
use futures::Stream;
use serde_json::Value;
use tracing::debug;

use crate::providers::pricing::ModelPricing;
use crate::providers::transform::kiro::KiroTransformer;
use crate::providers::transformer::{ProviderResponseMeta, ProviderTransformer, StreamState};
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

// ---------------------------------------------------------------------------
// KiroProvider
// ---------------------------------------------------------------------------

/// LLM provider that communicates through the Kiro gateway client.
///
/// The `KiroClient` handles authentication (refresh tokens, AWS SSO OIDC)
/// and API communication internally. This provider wraps it with format
/// conversion (via `KiroTransformer`) between OpenAI types and Anthropic
/// Messages types.
pub struct KiroProvider {
    client: Arc<kiro_gateway::KiroClient>,
    transformer: KiroTransformer,
}

impl KiroProvider {
    /// Create a new Kiro provider wrapping an already-built KiroClient.
    pub fn new(client: kiro_gateway::KiroClient) -> Self {
        Self {
            client: Arc::new(client),
            transformer: KiroTransformer::new(),
        }
    }
}

/// Convert a kiro-gateway error into a ProviderError.
fn convert_kiro_error(err: kiro_gateway::Error) -> ProviderError {
    match err {
        kiro_gateway::Error::NotAuthenticated | kiro_gateway::Error::TokenExpired => {
            ProviderError::NoToken {
                provider: "kiro".to_string(),
            }
        }
        kiro_gateway::Error::RefreshFailed(msg) => ProviderError::NoToken {
            provider: format!("kiro: {msg}"),
        },
        kiro_gateway::Error::Api { status, message } => ProviderError::Api { status, message },
        kiro_gateway::Error::RateLimited { retry_after } => ProviderError::RateLimited {
            retry_after_secs: retry_after.map(|d| d.as_secs()).unwrap_or(60),
            retry_after,
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
            // 1. Transform: OpenAI → Anthropic JSON via KiroTransformer
            let body: Value = self.transformer.transform_request(&request)?;
            debug!(body = %body, "Kiro request body");

            // 2. Deserialize into typed MessagesRequest for the kiro-gateway client
            let kiro_req: kiro_gateway::MessagesRequest =
                serde_json::from_value(body).map_err(|e| {
                    ProviderError::Other(format!("Failed to serialize Kiro request: {e}"))
                })?;

            // 3. Send via kiro-gateway (handles auth, model resolution, etc.)
            let response = self
                .client
                .send_messages(kiro_req)
                .await
                .map_err(convert_kiro_error)?;

            // 4. Serialize the typed response back to JSON
            let response_value = serde_json::to_value(&response).map_err(|e| {
                ProviderError::ResponseParsing(format!(
                    "Failed to serialize Kiro response: {e}"
                ))
            })?;

            // 5. Transform: Anthropic JSON → OpenAI via KiroTransformer
            let meta = ProviderResponseMeta::default();
            self.transformer.transform_response(response_value, &meta)
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
            let mut body: Value = self.transformer.transform_request(&request)?;
            // Ensure stream is true for the kiro-gateway client
            body.as_object_mut()
                .map(|o| o.insert("stream".to_string(), Value::Bool(true)));

            // 2. Deserialize into typed MessagesRequest
            let kiro_req: kiro_gateway::MessagesRequest =
                serde_json::from_value(body).map_err(|e| {
                    ProviderError::Other(format!("Failed to serialize Kiro request: {e}"))
                })?;

            // 3. Send streaming request via kiro-gateway
            let kiro_stream = self
                .client
                .send_messages_stream(kiro_req)
                .await
                .map_err(convert_kiro_error)?;

            // 4. Create a stream state processor from KiroTransformer
            let model = request.model.clone();
            let mut stream_state = self.transformer.new_stream_state(&model);

            // 5. Map typed StreamEvents through KiroStreamState
            let event_stream = kiro_stream.filter_map(move |result| {
                let chunk = match result {
                    Ok(event) => {
                        // Serialize the typed event to JSON for the stream state processor
                        match serde_json::to_string(&event) {
                            Ok(json) => match stream_state.process_event(&json) {
                                Ok(Some(chunk)) => Some(Ok(chunk)),
                                Ok(None) => None,
                                Err(e) => Some(Err(e)),
                            },
                            Err(e) => Some(Err(ProviderError::ResponseParsing(format!(
                                "Failed to serialize stream event: {e}"
                            )))),
                        }
                    }
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
        // All models should have the kiro: prefix.
        for m in SUPPORTED_MODELS {
            assert!(m.starts_with("kiro:"), "Model {} should start with kiro:", m);
        }
    }

    #[test]
    fn test_convert_kiro_error_auth() {
        let err = convert_kiro_error(kiro_gateway::Error::NotAuthenticated);
        assert!(matches!(err, ProviderError::NoToken { .. }));

        let err = convert_kiro_error(kiro_gateway::Error::TokenExpired);
        assert!(matches!(err, ProviderError::NoToken { .. }));
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
            ProviderError::RateLimited {
                retry_after_secs, ..
            } => {
                assert_eq!(retry_after_secs, 30);
            }
            _ => panic!("Expected RateLimited"),
        }
    }

    #[test]
    fn test_convert_kiro_error_refresh_failed() {
        let err = convert_kiro_error(kiro_gateway::Error::RefreshFailed("expired".into()));
        match err {
            ProviderError::NoToken { provider } => {
                assert!(provider.contains("kiro"));
                assert!(provider.contains("expired"));
            }
            _ => panic!("Expected NoToken"),
        }
    }

    #[test]
    fn test_convert_kiro_error_stream() {
        let err = convert_kiro_error(kiro_gateway::Error::Stream("broken pipe".into()));
        assert!(matches!(err, ProviderError::Stream(msg) if msg == "broken pipe"));
    }

    /// Verify that KiroTransformer produces JSON that round-trips through
    /// kiro_gateway::MessagesRequest without error.
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
        let kiro_req: kiro_gateway::MessagesRequest =
            serde_json::from_value(body).expect("Should deserialize into MessagesRequest");

        assert_eq!(kiro_req.model, "claude-sonnet-4");
        assert_eq!(kiro_req.max_tokens, 4096);
        assert_eq!(kiro_req.temperature, Some(0.7));
        assert_eq!(kiro_req.messages.len(), 1); // Only user message; system is separate.
        assert!(kiro_req.system.is_some());
    }

    /// Verify that a kiro_gateway::MessagesResponse round-trips through
    /// KiroTransformer::transform_response into the expected OpenAI format.
    #[test]
    fn test_transform_response_roundtrip() {
        let transformer = KiroTransformer::new();
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

        let response_value = serde_json::to_value(&resp).unwrap();
        let meta = ProviderResponseMeta::default();
        let chat_resp = transformer
            .transform_response(response_value, &meta)
            .unwrap();

        assert_eq!(chat_resp.id, "msg_123");
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

    /// Verify that tool use responses round-trip correctly.
    #[test]
    fn test_transform_response_tool_use_roundtrip() {
        let transformer = KiroTransformer::new();
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

        let response_value = serde_json::to_value(&resp).unwrap();
        let meta = ProviderResponseMeta::default();
        let chat_resp = transformer
            .transform_response(response_value, &meta)
            .unwrap();

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

    /// Verify that request with tools round-trips through MessagesRequest.
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
        let kiro_req: kiro_gateway::MessagesRequest =
            serde_json::from_value(body).expect("Should deserialize into MessagesRequest");

        assert!(kiro_req.tools.is_some());
        let tools = kiro_req.tools.unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "search");
        assert_eq!(tools[0].description, Some("Search the web".to_string()));
    }

    /// Verify that stream events round-trip through KiroStreamState.
    #[test]
    fn test_stream_event_roundtrip() {
        let transformer = KiroTransformer::new();
        let mut state = transformer.new_stream_state("kiro:auto");

        // message_start event
        let event = kiro_gateway::StreamEvent::MessageStart {
            message: kiro_gateway::models::stream::PartialMessage {
                id: "msg_1".into(),
                message_type: "message".into(),
                role: "assistant".into(),
                model: "claude-sonnet-4".into(),
                usage: kiro_gateway::Usage::default(),
            },
        };
        let json = serde_json::to_string(&event).unwrap();
        let chunk = state.process_event(&json).unwrap();
        assert!(chunk.is_some());
        let chunk = chunk.unwrap();
        assert_eq!(chunk.choices[0].delta.role, Some("assistant".to_string()));

        // text delta event
        let event = kiro_gateway::StreamEvent::ContentBlockDelta {
            index: 0,
            delta: kiro_gateway::ContentDelta::TextDelta {
                text: "Hello".into(),
            },
        };
        let json = serde_json::to_string(&event).unwrap();
        let chunk = state.process_event(&json).unwrap().unwrap();
        assert_eq!(chunk.choices[0].delta.content, Some("Hello".to_string()));
    }

    /// Verify that thinking responses round-trip correctly.
    #[test]
    fn test_transform_response_thinking_roundtrip() {
        let transformer = KiroTransformer::new();
        let resp = kiro_gateway::MessagesResponse {
            id: "msg_think".into(),
            response_type: "message".into(),
            role: "assistant".into(),
            content: vec![
                kiro_gateway::ResponseContentBlock::Thinking {
                    thinking: "Let me think about this...".into(),
                },
                kiro_gateway::ResponseContentBlock::Text {
                    text: "The answer is 42.".into(),
                },
            ],
            model: "claude-sonnet-4".into(),
            stop_reason: Some(kiro_gateway::StopReason::EndTurn),
            stop_sequence: None,
            usage: kiro_gateway::Usage {
                input_tokens: 20,
                output_tokens: 30,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            },
        };

        let response_value = serde_json::to_value(&resp).unwrap();
        let meta = ProviderResponseMeta::default();
        let chat_resp = transformer
            .transform_response(response_value, &meta)
            .unwrap();

        assert_eq!(
            chat_resp.choices[0].message.content,
            Some("The answer is 42.".to_string())
        );
        assert_eq!(
            chat_resp.choices[0].message.reasoning_content,
            Some("Let me think about this...".to_string())
        );
    }
}
