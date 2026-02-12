//! Anthropic Messages API request types.
//!
//! This module provides the request types for the Messages API,
//! including `MessagesRequest`, `Message`, `Role`, and related types.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::content::ContentBlock;
use super::tools::{Tool, ToolChoice};

/// A request to the Messages API.
///
/// This struct represents the complete request format for the Anthropic Messages API.
/// When sent through the proxy, it is converted to Google Generative AI format internally.
///
/// # Example
///
/// ```rust
/// use gaud::providers::gemini::models::{MessagesRequest, Message, Role};
///
/// let request = MessagesRequest::builder()
///     .model("claude-sonnet-4-5-thinking")
///     .max_tokens(1024)
///     .message(Message::user("Hello, Claude!"))
///     .build();
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MessagesRequest {
    /// The model to use for generation.
    pub model: String,

    /// The messages in the conversation.
    pub messages: Vec<Message>,

    /// Maximum number of tokens to generate.
    pub max_tokens: u32,

    /// System prompt to set context for the conversation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<SystemPrompt>,

    /// Sampling temperature (0.0 to 1.0).
    /// Higher values make output more random, lower values more deterministic.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,

    /// Top-p (nucleus) sampling parameter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,

    /// Top-k sampling parameter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,

    /// Stop sequences that will end generation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_sequences: Option<Vec<String>>,

    /// Tools available for the model to use.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Tool>>,

    /// How the model should choose tools.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,

    /// Configuration for thinking/reasoning output.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingConfig>,

    /// Whether to stream the response.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,

    /// Request metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

impl MessagesRequest {
    /// Create a new request builder.
    pub fn builder() -> MessagesRequestBuilder {
        MessagesRequestBuilder::default()
    }

    /// Create a simple request with the given model, max_tokens, and a user message.
    pub fn simple(model: impl Into<String>, max_tokens: u32, content: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            messages: vec![Message::user(content)],
            max_tokens,
            system: None,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: None,
            tools: None,
            tool_choice: None,
            thinking: None,
            stream: None,
            metadata: None,
        }
    }

    /// Check if this request has streaming enabled.
    pub fn is_streaming(&self) -> bool {
        self.stream.unwrap_or(false)
    }

    /// Check if this request has tools defined.
    pub fn has_tools(&self) -> bool {
        self.tools.as_ref().is_some_and(|t| !t.is_empty())
    }

    /// Check if this request has thinking enabled.
    pub fn has_thinking(&self) -> bool {
        self.thinking.is_some()
    }

    /// Get the thinking budget if configured.
    pub fn thinking_budget(&self) -> Option<u32> {
        self.thinking.as_ref().map(|t| t.budget_tokens)
    }
}

impl Default for MessagesRequest {
    fn default() -> Self {
        Self {
            model: String::new(),
            messages: Vec::new(),
            max_tokens: 1024,
            system: None,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: None,
            tools: None,
            tool_choice: None,
            thinking: None,
            stream: None,
            metadata: None,
        }
    }
}

/// Builder for constructing `MessagesRequest` instances.
#[derive(Debug, Default)]
pub struct MessagesRequestBuilder {
    model: Option<String>,
    messages: Vec<Message>,
    max_tokens: Option<u32>,
    system: Option<SystemPrompt>,
    temperature: Option<f32>,
    top_p: Option<f32>,
    top_k: Option<u32>,
    stop_sequences: Option<Vec<String>>,
    tools: Option<Vec<Tool>>,
    tool_choice: Option<ToolChoice>,
    thinking: Option<ThinkingConfig>,
    stream: Option<bool>,
    metadata: Option<Value>,
}

impl MessagesRequestBuilder {
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

    /// Add multiple messages to the conversation.
    pub fn messages(mut self, messages: impl IntoIterator<Item = Message>) -> Self {
        self.messages.extend(messages);
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

    /// Set the system prompt with blocks (for caching).
    pub fn system_blocks(mut self, blocks: Vec<SystemBlock>) -> Self {
        self.system = Some(SystemPrompt::Blocks(blocks));
        self
    }

    /// Set the sampling temperature.
    pub fn temperature(mut self, temperature: f32) -> Self {
        self.temperature = Some(temperature);
        self
    }

    /// Set the top_p parameter.
    pub fn top_p(mut self, top_p: f32) -> Self {
        self.top_p = Some(top_p);
        self
    }

    /// Set the top_k parameter.
    pub fn top_k(mut self, top_k: u32) -> Self {
        self.top_k = Some(top_k);
        self
    }

    /// Set stop sequences.
    pub fn stop_sequences(mut self, sequences: Vec<String>) -> Self {
        self.stop_sequences = Some(sequences);
        self
    }

    /// Add a stop sequence.
    pub fn stop_sequence(mut self, sequence: impl Into<String>) -> Self {
        self.stop_sequences
            .get_or_insert_with(Vec::new)
            .push(sequence.into());
        self
    }

    /// Set the available tools.
    pub fn tools(mut self, tools: Vec<Tool>) -> Self {
        self.tools = Some(tools);
        self
    }

    /// Add a tool.
    pub fn tool(mut self, tool: Tool) -> Self {
        self.tools.get_or_insert_with(Vec::new).push(tool);
        self
    }

    /// Set the tool choice.
    pub fn tool_choice(mut self, choice: ToolChoice) -> Self {
        self.tool_choice = Some(choice);
        self
    }

    /// Enable thinking with the specified token budget.
    pub fn thinking(mut self, budget_tokens: u32) -> Self {
        self.thinking = Some(ThinkingConfig { budget_tokens });
        self
    }

    /// Enable streaming.
    pub fn stream(mut self) -> Self {
        self.stream = Some(true);
        self
    }

    /// Disable streaming.
    pub fn no_stream(mut self) -> Self {
        self.stream = Some(false);
        self
    }

    /// Set request metadata.
    pub fn metadata(mut self, metadata: Value) -> Self {
        self.metadata = Some(metadata);
        self
    }

    /// Build the request.
    ///
    /// # Panics
    ///
    /// Panics if required fields (model, max_tokens) are not set.
    pub fn build(self) -> MessagesRequest {
        MessagesRequest {
            model: self.model.expect("model is required"),
            messages: self.messages,
            max_tokens: self.max_tokens.expect("max_tokens is required"),
            system: self.system,
            temperature: self.temperature,
            top_p: self.top_p,
            top_k: self.top_k,
            stop_sequences: self.stop_sequences,
            tools: self.tools,
            tool_choice: self.tool_choice,
            thinking: self.thinking,
            stream: self.stream,
            metadata: self.metadata,
        }
    }
}

/// A message in a conversation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Message {
    /// The role of the message sender.
    pub role: Role,

    /// The content of the message.
    pub content: MessageContent,
}

impl Message {
    /// Create a user message with text content.
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: MessageContent::Text(content.into()),
        }
    }

    /// Create a user message with content blocks.
    pub fn user_blocks(blocks: Vec<ContentBlock>) -> Self {
        Self {
            role: Role::User,
            content: MessageContent::Blocks(blocks),
        }
    }

    /// Create an assistant message with text content.
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: MessageContent::Text(content.into()),
        }
    }

    /// Create an assistant message with content blocks.
    pub fn assistant_blocks(blocks: Vec<ContentBlock>) -> Self {
        Self {
            role: Role::Assistant,
            content: MessageContent::Blocks(blocks),
        }
    }

    /// Check if this is a user message.
    pub fn is_user(&self) -> bool {
        self.role == Role::User
    }

    /// Check if this is an assistant message.
    pub fn is_assistant(&self) -> bool {
        self.role == Role::Assistant
    }

    /// Get the text content if the message is simple text.
    pub fn as_text(&self) -> Option<&str> {
        match &self.content {
            MessageContent::Text(text) => Some(text),
            _ => None,
        }
    }

    /// Get the content blocks if the message has blocks.
    pub fn as_blocks(&self) -> Option<&[ContentBlock]> {
        match &self.content {
            MessageContent::Blocks(blocks) => Some(blocks),
            _ => None,
        }
    }

    /// Get all text content from this message.
    ///
    /// For text messages, returns the text directly.
    /// For block messages, concatenates all text blocks.
    pub fn text(&self) -> String {
        match &self.content {
            MessageContent::Text(text) => text.clone(),
            MessageContent::Blocks(blocks) => blocks
                .iter()
                .filter_map(|b| b.as_text())
                .collect::<Vec<_>>()
                .join(""),
        }
    }

    /// Check if this message contains any tool use blocks.
    pub fn has_tool_use(&self) -> bool {
        match &self.content {
            MessageContent::Text(_) => false,
            MessageContent::Blocks(blocks) => blocks.iter().any(|b| b.is_tool_use()),
        }
    }

    /// Check if this message contains any tool result blocks.
    pub fn has_tool_result(&self) -> bool {
        match &self.content {
            MessageContent::Text(_) => false,
            MessageContent::Blocks(blocks) => blocks.iter().any(|b| b.is_tool_result()),
        }
    }

    /// Check if this message contains any thinking blocks.
    pub fn has_thinking(&self) -> bool {
        match &self.content {
            MessageContent::Text(_) => false,
            MessageContent::Blocks(blocks) => blocks.iter().any(|b| b.is_thinking()),
        }
    }
}

/// Role of a message sender.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// User message.
    User,
    /// Assistant message.
    Assistant,
}

impl Role {
    /// Check if this is the user role.
    pub fn is_user(&self) -> bool {
        matches!(self, Role::User)
    }

    /// Check if this is the assistant role.
    pub fn is_assistant(&self) -> bool {
        matches!(self, Role::Assistant)
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Role::User => write!(f, "user"),
            Role::Assistant => write!(f, "assistant"),
        }
    }
}

/// Content of a message.
///
/// Messages can contain either simple text or multiple content blocks.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum MessageContent {
    /// Simple text content.
    Text(String),
    /// Multiple content blocks.
    Blocks(Vec<ContentBlock>),
}

impl MessageContent {
    /// Check if this is text content.
    pub fn is_text(&self) -> bool {
        matches!(self, MessageContent::Text(_))
    }

    /// Check if this is block content.
    pub fn is_blocks(&self) -> bool {
        matches!(self, MessageContent::Blocks(_))
    }

    /// Get the text if this is text content.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            MessageContent::Text(text) => Some(text),
            _ => None,
        }
    }

    /// Get the blocks if this is block content.
    pub fn as_blocks(&self) -> Option<&[ContentBlock]> {
        match self {
            MessageContent::Blocks(blocks) => Some(blocks),
            _ => None,
        }
    }
}

impl From<String> for MessageContent {
    fn from(text: String) -> Self {
        MessageContent::Text(text)
    }
}

impl From<&str> for MessageContent {
    fn from(text: &str) -> Self {
        MessageContent::Text(text.to_string())
    }
}

impl From<Vec<ContentBlock>> for MessageContent {
    fn from(blocks: Vec<ContentBlock>) -> Self {
        MessageContent::Blocks(blocks)
    }
}

/// System prompt for the conversation.
///
/// Can be either a simple string or blocks (for prompt caching).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum SystemPrompt {
    /// Simple text system prompt.
    Text(String),
    /// System prompt with blocks (for caching support).
    Blocks(Vec<SystemBlock>),
}

impl SystemPrompt {
    /// Create a text system prompt.
    pub fn text(text: impl Into<String>) -> Self {
        SystemPrompt::Text(text.into())
    }

    /// Create a blocks system prompt.
    pub fn blocks(blocks: Vec<SystemBlock>) -> Self {
        SystemPrompt::Blocks(blocks)
    }

    /// Check if this is a text prompt.
    pub fn is_text(&self) -> bool {
        matches!(self, SystemPrompt::Text(_))
    }

    /// Check if this is a blocks prompt.
    pub fn is_blocks(&self) -> bool {
        matches!(self, SystemPrompt::Blocks(_))
    }

    /// Get the text if this is a text prompt.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            SystemPrompt::Text(text) => Some(text),
            _ => None,
        }
    }

    /// Get all text content from this system prompt.
    pub fn text_content(&self) -> String {
        match self {
            SystemPrompt::Text(text) => text.clone(),
            SystemPrompt::Blocks(blocks) => blocks
                .iter()
                .map(|b| {
                    let SystemBlock::Text { text, .. } = b;
                    text.as_str()
                })
                .collect::<Vec<_>>()
                .join(""),
        }
    }
}

impl From<String> for SystemPrompt {
    fn from(text: String) -> Self {
        SystemPrompt::Text(text)
    }
}

impl From<&str> for SystemPrompt {
    fn from(text: &str) -> Self {
        SystemPrompt::Text(text.to_string())
    }
}

/// A block in a system prompt.
///
/// System blocks support text content with optional cache control.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SystemBlock {
    /// Text content with optional cache control.
    Text {
        /// The text content.
        text: String,
        /// Cache control settings.
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
}

impl SystemBlock {
    /// Create a text block.
    pub fn text(text: impl Into<String>) -> Self {
        SystemBlock::Text {
            text: text.into(),
            cache_control: None,
        }
    }

    /// Create a text block with ephemeral cache control.
    pub fn text_ephemeral(text: impl Into<String>) -> Self {
        SystemBlock::Text {
            text: text.into(),
            cache_control: Some(CacheControl::ephemeral()),
        }
    }
}

/// Cache control settings for prompt caching.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CacheControl {
    /// The cache type.
    #[serde(rename = "type")]
    pub cache_type: String,
}

impl CacheControl {
    /// Create an ephemeral cache control.
    pub fn ephemeral() -> Self {
        Self {
            cache_type: "ephemeral".to_string(),
        }
    }
}

/// Configuration for thinking/reasoning output.
///
/// When enabled, thinking models will include their reasoning process
/// in the response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ThinkingConfig {
    /// Maximum tokens to use for thinking.
    /// Higher values allow for more complex reasoning.
    pub budget_tokens: u32,
}

impl ThinkingConfig {
    /// Create a new thinking configuration with the given budget.
    pub fn new(budget_tokens: u32) -> Self {
        Self { budget_tokens }
    }
}

impl Default for ThinkingConfig {
    fn default() -> Self {
        Self {
            budget_tokens: 10000,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_simple_request() {
        let request = MessagesRequest::simple("claude-sonnet-4-5-thinking", 1024, "Hello!");

        assert_eq!(request.model, "claude-sonnet-4-5-thinking");
        assert_eq!(request.max_tokens, 1024);
        assert_eq!(request.messages.len(), 1);
        assert!(request.messages[0].is_user());
    }

    #[test]
    fn test_request_builder() {
        let request = MessagesRequest::builder()
            .model("claude-sonnet-4-5-thinking")
            .max_tokens(2048)
            .system("You are a helpful assistant.")
            .user_message("Hello!")
            .assistant_message("Hi there!")
            .user_message("How are you?")
            .temperature(0.7)
            .build();

        assert_eq!(request.model, "claude-sonnet-4-5-thinking");
        assert_eq!(request.max_tokens, 2048);
        assert_eq!(request.messages.len(), 3);
        assert_eq!(request.temperature, Some(0.7));
        assert!(request.system.is_some());
    }

    #[test]
    fn test_request_builder_with_tools() {
        let tool = Tool::new(
            "get_weather",
            "Get weather",
            json!({"type": "object", "properties": {}}),
        );

        let request = MessagesRequest::builder()
            .model("claude-sonnet-4-5")
            .max_tokens(1024)
            .tool(tool)
            .tool_choice(ToolChoice::Auto)
            .build();

        assert!(request.has_tools());
        assert_eq!(request.tools.unwrap().len(), 1);
    }

    #[test]
    fn test_request_builder_with_thinking() {
        let request = MessagesRequest::builder()
            .model("claude-sonnet-4-5-thinking")
            .max_tokens(1024)
            .thinking(10000)
            .build();

        assert!(request.has_thinking());
        assert_eq!(request.thinking_budget(), Some(10000));
    }

    #[test]
    fn test_request_serialization() {
        let request = MessagesRequest::simple("claude-sonnet-4-5", 1024, "Hello!");

        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["model"], "claude-sonnet-4-5");
        assert_eq!(json["max_tokens"], 1024);
        assert!(json["messages"].is_array());
    }

    #[test]
    fn test_request_optional_fields_omitted() {
        let request = MessagesRequest::simple("claude-sonnet-4-5", 1024, "Hello!");
        let json = serde_json::to_string(&request).unwrap();

        // Optional fields should not appear in JSON
        assert!(!json.contains("temperature"));
        assert!(!json.contains("top_p"));
        assert!(!json.contains("tools"));
        assert!(!json.contains("thinking"));
    }

    #[test]
    fn test_message_user() {
        let msg = Message::user("Hello!");
        assert!(msg.is_user());
        assert!(!msg.is_assistant());
        assert_eq!(msg.as_text(), Some("Hello!"));
        assert_eq!(msg.text(), "Hello!");
    }

    #[test]
    fn test_message_assistant() {
        let msg = Message::assistant("Hi there!");
        assert!(msg.is_assistant());
        assert!(!msg.is_user());
    }

    #[test]
    fn test_message_with_blocks() {
        let msg = Message::user_blocks(vec![
            ContentBlock::text("Hello!"),
            ContentBlock::image_url("https://example.com/img.png"),
        ]);

        assert!(msg.is_user());
        assert!(msg.as_blocks().is_some());
        assert_eq!(msg.as_blocks().unwrap().len(), 2);
        assert_eq!(msg.text(), "Hello!");
    }

    #[test]
    fn test_message_has_tool_use() {
        let msg_text = Message::user("Hello");
        assert!(!msg_text.has_tool_use());

        let msg_with_tool =
            Message::assistant_blocks(vec![ContentBlock::tool_use("t1", "calc", json!({"x": 1}))]);
        assert!(msg_with_tool.has_tool_use());
    }

    #[test]
    fn test_message_has_tool_result() {
        let msg = Message::user_blocks(vec![ContentBlock::tool_result("t1", "result")]);
        assert!(msg.has_tool_result());
    }

    #[test]
    fn test_message_has_thinking() {
        let msg = Message::assistant_blocks(vec![ContentBlock::thinking("Let me think...", None)]);
        assert!(msg.has_thinking());
    }

    #[test]
    fn test_role_display() {
        assert_eq!(Role::User.to_string(), "user");
        assert_eq!(Role::Assistant.to_string(), "assistant");
    }

    #[test]
    fn test_role_serialization() {
        assert_eq!(serde_json::to_string(&Role::User).unwrap(), r#""user""#);
        assert_eq!(
            serde_json::to_string(&Role::Assistant).unwrap(),
            r#""assistant""#
        );
    }

    #[test]
    fn test_message_content_text() {
        let content = MessageContent::Text("Hello".to_string());
        assert!(content.is_text());
        assert_eq!(content.as_text(), Some("Hello"));

        let json = serde_json::to_value(&content).unwrap();
        assert_eq!(json, "Hello");
    }

    #[test]
    fn test_message_content_blocks() {
        let content = MessageContent::Blocks(vec![ContentBlock::text("Hello")]);
        assert!(content.is_blocks());
        assert!(content.as_blocks().is_some());

        let json = serde_json::to_value(&content).unwrap();
        assert!(json.is_array());
    }

    #[test]
    fn test_message_content_from_impls() {
        let from_string: MessageContent = "hello".to_string().into();
        assert!(from_string.is_text());

        let from_str: MessageContent = "hello".into();
        assert!(from_str.is_text());

        let from_vec: MessageContent = vec![ContentBlock::text("hello")].into();
        assert!(from_vec.is_blocks());
    }

    #[test]
    fn test_system_prompt_text() {
        let prompt = SystemPrompt::text("You are helpful.");
        assert!(prompt.is_text());
        assert_eq!(prompt.as_text(), Some("You are helpful."));
        assert_eq!(prompt.text_content(), "You are helpful.");

        let json = serde_json::to_value(&prompt).unwrap();
        assert_eq!(json, "You are helpful.");
    }

    #[test]
    fn test_system_prompt_blocks() {
        let prompt = SystemPrompt::blocks(vec![
            SystemBlock::text("Part 1"),
            SystemBlock::text("Part 2"),
        ]);

        assert!(prompt.is_blocks());
        assert_eq!(prompt.text_content(), "Part 1Part 2");
    }

    #[test]
    fn test_system_block_with_cache_control() {
        let block = SystemBlock::text_ephemeral("Cached content");

        let json = serde_json::to_value(&block).unwrap();
        assert_eq!(json["type"], "text");
        assert_eq!(json["text"], "Cached content");
        assert_eq!(json["cache_control"]["type"], "ephemeral");
    }

    #[test]
    fn test_thinking_config() {
        let config = ThinkingConfig::new(5000);
        assert_eq!(config.budget_tokens, 5000);

        let default_config = ThinkingConfig::default();
        assert_eq!(default_config.budget_tokens, 10000);

        let json = serde_json::to_value(&config).unwrap();
        assert_eq!(json["budget_tokens"], 5000);
    }

    #[test]
    fn test_request_deserialization() {
        let json = r#"{
            "model": "claude-sonnet-4-5",
            "max_tokens": 1024,
            "messages": [
                {"role": "user", "content": "Hello!"}
            ],
            "temperature": 0.5
        }"#;

        let request: MessagesRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.model, "claude-sonnet-4-5");
        assert_eq!(request.max_tokens, 1024);
        assert_eq!(request.messages.len(), 1);
        assert_eq!(request.temperature, Some(0.5));
    }

    #[test]
    fn test_request_roundtrip() {
        let original = MessagesRequest::builder()
            .model("claude-sonnet-4-5")
            .max_tokens(2048)
            .system("Be helpful")
            .user_message("Hello")
            .temperature(0.7)
            .top_p(0.9)
            .thinking(8000)
            .stream()
            .build();

        let serialized = serde_json::to_string(&original).unwrap();
        let deserialized: MessagesRequest = serde_json::from_str(&serialized).unwrap();

        assert_eq!(original, deserialized);
    }

    #[test]
    fn test_message_deserialization_text() {
        let json = r#"{"role": "user", "content": "Hello"}"#;
        let msg: Message = serde_json::from_str(json).unwrap();
        assert!(msg.is_user());
        assert_eq!(msg.as_text(), Some("Hello"));
    }

    #[test]
    fn test_message_deserialization_blocks() {
        let json = r#"{"role": "assistant", "content": [{"type": "text", "text": "Hi"}]}"#;
        let msg: Message = serde_json::from_str(json).unwrap();
        assert!(msg.is_assistant());
        assert!(msg.as_blocks().is_some());
    }
}
