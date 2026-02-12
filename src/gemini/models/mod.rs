//! Data models for the Anthropic Messages API.
//!
//! This module provides all the types needed to construct requests
//! and handle responses from the Messages API. The types follow
//! the Anthropic API specification and are converted internally
//! to Google Generative AI format.
//!
//! # Request Types
//!
//! - [`MessagesRequest`] - The main request type
//! - [`Message`] - A message in a conversation
//! - [`Role`] - User or Assistant role
//! - [`MessageContent`] - Text or block content
//! - [`SystemPrompt`] - System instructions
//! - [`ThinkingConfig`] - Extended thinking configuration
//!
//! # Response Types
//!
//! - [`MessagesResponse`] - The response from the API
//! - [`Usage`] - Token usage information
//! - [`StopReason`] - Why generation stopped
//!
//! # Content Types
//!
//! - [`ContentBlock`] - Text, tool use, tool result, thinking, image, document
//! - [`ImageSource`] - Base64 or URL image source
//! - [`DocumentSource`] - Base64 document source
//! - [`ToolResultContent`] - Text or block content for tool results
//!
//! # Tool Types
//!
//! - [`Tool`] - A tool definition with name, description, and schema
//! - [`ToolChoice`] - How the model should use tools
//!
//! # Streaming Types
//!
//! - [`StreamEvent`] - Server-sent events during streaming
//! - [`ContentDelta`] - Incremental content updates
//! - [`MessageDelta`] - Final message metadata
//! - [`PartialMessage`] - Partial message at stream start
//! - [`StreamError`] - Error during streaming
//!
//! # Example
//!
//! ```rust
//! use gaud::gemini::models::{
//!     MessagesRequest, Message, Role, ContentBlock, Tool, ToolChoice,
//! };
//! use serde_json::json;
//!
//! // Build a request with tools
//! let request = MessagesRequest::builder()
//!     .model("claude-sonnet-4-5")
//!     .max_tokens(1024)
//!     .user_message("What's the weather in Tokyo?")
//!     .tool(Tool::new(
//!         "get_weather",
//!         "Get current weather for a location",
//!         json!({
//!             "type": "object",
//!             "properties": {
//!                 "location": {"type": "string", "description": "City name"}
//!             },
//!             "required": ["location"]
//!         }),
//!     ))
//!     .tool_choice(ToolChoice::Auto)
//!     .build();
//! ```

pub mod content;
pub mod request;
pub mod response;
pub mod stream;
pub mod tools;

// Internal Google API types (not part of public API)
pub(crate) mod google;

// Re-export all public types
pub use content::{ContentBlock, DocumentSource, ImageSource, ToolResultContent};
pub use request::{
    CacheControl, Message, MessageContent, MessagesRequest, MessagesRequestBuilder, Role,
    SystemBlock, SystemPrompt, ThinkingConfig,
};
pub use response::{MessagesResponse, StopReason, Usage};
pub use stream::{ContentDelta, MessageDelta, PartialMessage, StreamError, StreamEvent};
pub use tools::{Tool, ToolChoice};
