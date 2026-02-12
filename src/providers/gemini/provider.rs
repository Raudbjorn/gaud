//! Gemini (Google) Provider
//!
//! Uses the `gemini` library to communicate with the Google Gemini API
//! via the Cloud Code API client.

use std::pin::Pin;

use futures::{Stream, StreamExt};
use tracing::warn;

use crate::providers::gemini::{
    client::CloudCodeClient,
    models::{
        ContentBlock, ContentDelta, Message, MessageContent, MessagesRequest, MessagesResponse,
        Role, StopReason, StreamEvent, SystemPrompt, Tool,
    },
};
use crate::providers::transform::gemini::{
    convert_request as to_google_req, convert_response as from_google_resp,
};

use crate::providers::pricing::ModelPricing;
use crate::providers::types::{
    ChatChunk, ChatRequest, ChatResponse, Choice, ChunkChoice, Delta, FunctionCall, MessageRole,
    ResponseMessage, ToolCall, Usage,
};
use crate::providers::{LlmProvider, ProviderError};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const SUPPORTED_MODELS: &[&str] = &[
    "gemini-2.5-flash",
    "gemini-2.5-pro",
    "gemini-2.0-flash",
    "gemini-1.5-flash",
    "gemini-1.5-pro",
];

// ---------------------------------------------------------------------------
// Gemini Provider
// ---------------------------------------------------------------------------

/// LLM provider that communicates with the Google Gemini API.
pub struct GeminiProvider {
    client: CloudCodeClient,
}

impl GeminiProvider {
    /// Create a new Gemini provider backed by the given token provider.
    pub fn new(token_provider: std::sync::Arc<dyn crate::auth::TokenProvider>) -> Self {
        let client = CloudCodeClient::new(token_provider);
        Self { client }
    }

    // -- Conversion Helpers -------------------------------------------------

    fn convert_request(&self, request: &ChatRequest) -> Result<MessagesRequest, ProviderError> {
        let mut messages = Vec::new();
        let mut system = None;

        for msg in &request.messages {
            match msg.role {
                MessageRole::System => {
                    // Concatenate system messages if multiple
                    let text = match &msg.content {
                        Some(crate::providers::types::MessageContent::Text(t)) => t.clone(),
                        Some(crate::providers::types::MessageContent::Parts(parts)) => parts
                            .iter()
                            .filter_map(|p| match p {
                                crate::providers::types::ContentPart::Text { text } => {
                                    Some(text.clone())
                                }
                                _ => None,
                            })
                            .collect::<Vec<_>>()
                            .join("\n"),
                        None => continue,
                    };

                    if let Some(SystemPrompt::Text(existing)) = &system {
                        system = Some(SystemPrompt::Text(format!("{}\n{}", existing, text)));
                    } else {
                        system = Some(SystemPrompt::Text(text));
                    }
                }
                MessageRole::User | MessageRole::Assistant | MessageRole::Tool => {
                    let role = if msg.role == MessageRole::User || msg.role == MessageRole::Tool {
                        Role::User
                    } else {
                        Role::Assistant
                    };

                    let mut blocks = Vec::new();

                    // Handle text/parts content
                    if let Some(content) = &msg.content {
                        match content {
                            crate::providers::types::MessageContent::Text(text) => {
                                blocks.push(ContentBlock::text(text));
                            }
                            crate::providers::types::MessageContent::Parts(msg_parts) => {
                                for part in msg_parts {
                                    match part {
                                        crate::providers::types::ContentPart::Text { text } => {
                                            blocks.push(ContentBlock::text(text));
                                        }
                                        crate::providers::types::ContentPart::ImageUrl {
                                            image_url,
                                        } => {
                                            // For now simpler adapter: just warn about images or try to pass URL
                                            warn!(
                                                "Image content not fully supported in adapter: {}",
                                                image_url.url
                                            );
                                            blocks.push(ContentBlock::text(format!(
                                                "[Image: {}]",
                                                image_url.url
                                            )));
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // Handle tool calls (Assistant only)
                    if msg.role == MessageRole::Assistant {
                        if let Some(calls) = &msg.tool_calls {
                            for call in calls {
                                if let Ok(args) = serde_json::from_str(&call.function.arguments) {
                                    blocks.push(ContentBlock::tool_use(
                                        call.id.clone(),
                                        call.function.name.clone(),
                                        args,
                                    ));
                                }
                            }
                        }
                    }

                    // Handle tool response (Tool/User)
                    if msg.role == MessageRole::Tool {
                        let tool_use_id = msg.tool_call_id.clone().unwrap_or_default();
                        let content_text = match &msg.content {
                            Some(crate::providers::types::MessageContent::Text(t)) => t.clone(),
                            _ => String::new(),
                        };
                        blocks.push(ContentBlock::tool_result(tool_use_id, content_text));
                    }

                    if !blocks.is_empty() {
                        messages.push(Message {
                            role,
                            content: MessageContent::Blocks(blocks),
                        });
                    }
                }
            }
        }

        // Tools
        let tools = if let Some(req_tools) = &request.tools {
            let mut methods = Vec::new();
            for t in req_tools {
                if t.r#type == "function" {
                    // We need to construct gemini::models::tools::Tool
                    // It expects name, description, input_schema.
                    // ChatRequest Tool is { type: "function", function: FunctionDef { name, description, parameters } }

                    if let Some(params) = &t.function.parameters {
                        methods.push(Tool::new(
                            t.function.name.clone(),
                            t.function.description.clone().unwrap_or_default(),
                            params.clone(),
                        ));
                    }
                }
            }
            if methods.is_empty() {
                None
            } else {
                Some(methods)
            }
        } else {
            None
        };

        Ok(MessagesRequest {
            model: request.model.clone(),
            messages,
            max_tokens: request.max_tokens.unwrap_or(4096),
            system,
            temperature: request.temperature,
            top_p: request.top_p,
            top_k: None,
            stop_sequences: request.stop.clone().map(|s| match s {
                crate::providers::types::StopSequence::Single(val) => vec![val],
                crate::providers::types::StopSequence::Multiple(vec) => vec,
            }),
            tools,
            tool_choice: None, // Simplified
            thinking: None,
            stream: Some(request.stream),
            metadata: None,
        })
    }

    fn convert_response(
        &self,
        resp: MessagesResponse,
        model: &str,
    ) -> Result<ChatResponse, ProviderError> {
        let created = chrono::Utc::now().timestamp();

        let mut content = None;
        let mut tool_calls = None;

        // Convert Anthropic content blocks back to OpenAI format
        let mut text_parts = Vec::new();
        let mut tcs = Vec::new();

        for block in resp.content {
            match block {
                ContentBlock::Text { text, .. } => text_parts.push(text),
                ContentBlock::ToolUse {
                    id, name, input, ..
                } => {
                    tcs.push(ToolCall {
                        index: Some(tcs.len() as u32),
                        id,
                        r#type: "function".to_string(),
                        function: FunctionCall {
                            name,
                            arguments: serde_json::to_string(&input).unwrap_or_default(),
                        },
                    });
                }
                _ => {}
            }
        }

        if !text_parts.is_empty() {
            content = Some(text_parts.join(""));
        }
        if !tcs.is_empty() {
            tool_calls = Some(tcs);
        }

        let finish_reason = resp.stop_reason.map(|r| match r {
            StopReason::EndTurn => "stop".to_string(),
            StopReason::MaxTokens => "length".to_string(),
            StopReason::StopSequence => "stop".to_string(),
            StopReason::ToolUse => "tool_calls".to_string(),
        });

        let choices = vec![Choice {
            index: 0,
            message: ResponseMessage {
                role: "assistant".to_string(),
                content,
                reasoning_content: None,
                tool_calls,
            },
            finish_reason,
        }];

        let usage = Usage {
            prompt_tokens: resp.usage.input_tokens,
            completion_tokens: resp.usage.output_tokens,
            total_tokens: resp.usage.input_tokens + resp.usage.output_tokens,
            prompt_tokens_details: None,
            completion_tokens_details: None,
        };

        Ok(ChatResponse {
            id: resp.id,
            object: "chat.completion".to_string(),
            created,
            model: model.to_string(),
            choices,
            usage,
        })
    }
}

// ---------------------------------------------------------------------------
// LlmProvider implementation
// ---------------------------------------------------------------------------

impl LlmProvider for GeminiProvider {
    fn id(&self) -> &str {
        "gemini"
    }

    fn name(&self) -> &str {
        "Gemini (Google)"
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
            if !self.supports_model(&request.model) {
                return Err(ProviderError::Other(format!(
                    "Unsupported Gemini model: {}",
                    request.model
                )));
            }

            let msg_req = self.convert_request(&request)?;
            let google_req = to_google_req(&msg_req);

            let google_resp = self
                .client
                .request(&msg_req.model, google_req)
                .await
                .map_err(|e| ProviderError::Api {
                    status: 500,
                    message: e.to_string(),
                })?;

            let msg_resp = from_google_resp(&google_resp, &msg_req.model);

            self.convert_response(msg_resp, &request.model)
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
            if !self.supports_model(&request.model) {
                return Err(ProviderError::Other(format!(
                    "Unsupported Gemini model: {}",
                    request.model
                )));
            }

            let msg_req = self.convert_request(&request)?;
            let google_req = to_google_req(&msg_req);

            let stream = self
                .client
                .request_stream(&msg_req.model, google_req)
                .await
                .map_err(|e| ProviderError::Api {
                    status: 500,
                    message: e.to_string(),
                })?;

            // Map the stream
            let mapped_stream = stream.map(move |result| {
                match result {
                    Ok(event) => {
                        let id = format!("chunk-{}", uuid::Uuid::new_v4()); // Should use message ID from event if available

                        let mut delta_content = None;
                        let mut finish_reason = None;
                        let delta_tool_calls = None;

                        match event {
                            StreamEvent::ContentBlockDelta { delta, .. } => match delta {
                                ContentDelta::TextDelta { text } => {
                                    delta_content = Some(text);
                                }
                                _ => {}
                            },
                            StreamEvent::MessageDelta { delta, .. } => {
                                if let Some(reason) = delta.stop_reason {
                                    finish_reason = Some(format!("{:?}", reason));
                                }
                            }
                            // TODO: Handle tool use streaming
                            _ => {}
                        }

                        if delta_content.is_none() && finish_reason.is_none() {
                            // Skip empty updates (keep-alives etc)
                            // But we need to return something or filter map?
                            // Since we return Item=Result, we can't skip easily without filter_map wrapper.
                            // Return empty chunk?
                            return Ok(ChatChunk {
                                id,
                                object: "chat.completion.chunk".to_string(),
                                created: chrono::Utc::now().timestamp(),
                                model: request.model.clone(),
                                choices: vec![],
                                usage: None,
                            });
                        }

                        Ok(ChatChunk {
                            id,
                            object: "chat.completion.chunk".to_string(),
                            created: chrono::Utc::now().timestamp(),
                            model: request.model.clone(),
                            choices: vec![ChunkChoice {
                                index: 0,
                                delta: Delta {
                                    role: Some("assistant".into()),
                                    content: delta_content,
                                    tool_calls: delta_tool_calls,
                                    reasoning_content: None,
                                },
                                finish_reason,
                            }],
                            usage: None,
                        })
                    }
                    Err(e) => Err(ProviderError::Stream(e.to_string())),
                }
            });

            Ok(Box::pin(mapped_stream)
                as Pin<
                    Box<dyn Stream<Item = Result<ChatChunk, ProviderError>> + Send>,
                >)
        })
    }

    fn health_check(&self) -> Pin<Box<dyn std::future::Future<Output = bool> + Send + '_>> {
        Box::pin(async move { self.client.is_authenticated().await.unwrap_or(false) })
    }

    fn pricing(&self) -> Vec<ModelPricing> {
        crate::providers::cost::CostCalculator::all()
            .into_iter()
            .filter(|p| p.provider == "gemini")
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{error::AuthError, TokenProvider};
    use std::sync::Arc;

    struct MockTokenProvider;

    #[async_trait::async_trait]
    impl TokenProvider for MockTokenProvider {
        async fn get_token(&self, _provider: &str) -> Result<String, AuthError> {
            Ok("mock_token".to_string())
        }
    }

    #[test]
    fn test_convert_request() {
        let provider = GeminiProvider::new(Arc::new(MockTokenProvider));

        let req = ChatRequest {
            model: "gemini-1.5-pro".to_string(),
            messages: vec![crate::providers::types::ChatMessage {
                role: MessageRole::User,
                content: Some(crate::providers::types::MessageContent::Text(
                    "Hello".to_string(),
                )),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            }],
            temperature: Some(0.5),
            max_tokens: Some(100),
            stream: false,
            top_p: None,
            stop: Some(crate::providers::types::StopSequence::Single(
                "stop".to_string(),
            )),
            tools: None,
            tool_choice: None,
            stream_options: None,
        };

        let msg_req = provider.convert_request(&req).unwrap();

        assert_eq!(msg_req.model, "gemini-1.5-pro");
        assert_eq!(msg_req.stop_sequences, Some(vec!["stop".to_string()]));
        assert_eq!(msg_req.messages.len(), 1);
        assert_eq!(msg_req.temperature, Some(0.5));
    }
}
