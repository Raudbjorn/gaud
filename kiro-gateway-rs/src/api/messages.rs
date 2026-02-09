//! Fluent Messages API request builder.

use futures::Stream;
use std::pin::Pin;

use crate::error::Result;
use crate::models::request::{
    ContentBlock, Message, MessageContent, MessagesRequest, Role, SystemPrompt, ThinkingConfig,
    Tool, ToolChoice,
};
use crate::models::response::MessagesResponse;
use crate::models::stream::StreamEvent;

/// Builder for Messages API requests.
///
/// ```rust,no_run
/// # use kiro_gateway::KiroClient;
/// # async fn example(client: &KiroClient) -> kiro_gateway::Result<()> {
/// let response = client.messages()
///     .model("claude-sonnet-4.5")
///     .max_tokens(1024)
///     .user_message("Hello!")
///     .send()
///     .await?;
/// # Ok(())
/// # }
/// ```
pub struct MessagesRequestBuilder<'a> {
    client: &'a crate::client::KiroClient,
    request: MessagesRequest,
}

impl<'a> MessagesRequestBuilder<'a> {
    /// Create a new builder.
    pub(crate) fn new(client: &'a crate::client::KiroClient) -> Self {
        Self {
            client,
            request: MessagesRequest {
                model: "auto".to_string(),
                max_tokens: 4096,
                messages: Vec::new(),
                system: None,
                tools: None,
                tool_choice: None,
                stream: false,
                temperature: None,
                top_p: None,
                stop_sequences: None,
                thinking: None,
            },
        }
    }

    /// Set the model (e.g., "claude-sonnet-4.5", "claude-haiku-4.5").
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.request.model = model.into();
        self
    }

    /// Set the maximum number of tokens to generate.
    pub fn max_tokens(mut self, max_tokens: u32) -> Self {
        self.request.max_tokens = max_tokens;
        self
    }

    /// Set the system prompt (plain text).
    pub fn system(mut self, system: impl Into<String>) -> Self {
        self.request.system = Some(SystemPrompt::Text(system.into()));
        self
    }

    /// Add a user message (plain text).
    pub fn user_message(mut self, content: impl Into<String>) -> Self {
        self.request.messages.push(Message {
            role: Role::User,
            content: MessageContent::Text(content.into()),
        });
        self
    }

    /// Add an assistant message (for multi-turn conversations).
    pub fn assistant_message(mut self, content: impl Into<String>) -> Self {
        self.request.messages.push(Message {
            role: Role::Assistant,
            content: MessageContent::Text(content.into()),
        });
        self
    }

    /// Add a message with content blocks (for images, tool results, etc.).
    pub fn message(mut self, role: Role, blocks: Vec<ContentBlock>) -> Self {
        self.request.messages.push(Message {
            role,
            content: MessageContent::Blocks(blocks),
        });
        self
    }

    /// Set the full messages list.
    pub fn messages(mut self, messages: Vec<Message>) -> Self {
        self.request.messages = messages;
        self
    }

    /// Add a tool definition.
    pub fn tool(mut self, name: impl Into<String>, description: impl Into<String>, input_schema: serde_json::Value) -> Self {
        let tools = self.request.tools.get_or_insert_with(Vec::new);
        tools.push(Tool {
            name: name.into(),
            description: Some(description.into()),
            input_schema,
        });
        self
    }

    /// Set all tool definitions at once.
    pub fn tools(mut self, tools: Vec<Tool>) -> Self {
        self.request.tools = Some(tools);
        self
    }

    /// Set the tool choice strategy.
    pub fn tool_choice(mut self, choice: ToolChoice) -> Self {
        self.request.tool_choice = Some(choice);
        self
    }

    /// Set the temperature (0.0-1.0).
    pub fn temperature(mut self, temp: f32) -> Self {
        self.request.temperature = Some(temp);
        self
    }

    /// Set top-p sampling.
    pub fn top_p(mut self, top_p: f32) -> Self {
        self.request.top_p = Some(top_p);
        self
    }

    /// Set stop sequences.
    pub fn stop_sequences(mut self, sequences: Vec<String>) -> Self {
        self.request.stop_sequences = Some(sequences);
        self
    }

    /// Enable extended thinking.
    pub fn thinking(mut self, budget_tokens: u32) -> Self {
        self.request.thinking = Some(ThinkingConfig {
            thinking_type: "enabled".to_string(),
            budget_tokens: Some(budget_tokens),
        });
        self
    }

    /// Send the request and get a complete response.
    pub async fn send(self) -> Result<MessagesResponse> {
        self.client.send_messages(self.request).await
    }

    /// Send the request and get a streaming response.
    pub async fn send_stream(
        mut self,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        self.request.stream = true;
        self.client.send_messages_stream(self.request).await
    }

    /// Get the built request without sending it.
    pub fn build(self) -> MessagesRequest {
        self.request
    }
}
