//! Streaming event types for the Messages API.
//!
//! This module provides types for handling Server-Sent Events (SSE)
//! from the streaming Messages API.

use serde::{Deserialize, Serialize};

use super::content::ContentBlock;
use super::request::Role;
use super::response::{StopReason, Usage};

/// A streaming event from the Messages API.
///
/// These events are sent via Server-Sent Events (SSE) when streaming
/// is enabled. Events come in a specific order:
///
/// 1. `MessageStart` - Beginning of the response
/// 2. `ContentBlockStart` - Start of each content block
/// 3. `ContentBlockDelta` - Incremental content updates (multiple)
/// 4. `ContentBlockStop` - End of each content block
/// 5. `MessageDelta` - Final message metadata (stop_reason, usage)
/// 6. `MessageStop` - End of the response
///
/// `Ping` events may be sent at any time to keep the connection alive.
/// `Error` events indicate an error occurred during streaming.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    /// Beginning of the message response.
    MessageStart {
        /// The partial message being started.
        message: PartialMessage,
    },

    /// Start of a content block.
    ContentBlockStart {
        /// Index of this content block (0-based).
        index: usize,
        /// The initial content block (may be empty or partial).
        content_block: ContentBlock,
    },

    /// Incremental update to a content block.
    ContentBlockDelta {
        /// Index of the content block being updated.
        index: usize,
        /// The delta to apply.
        delta: ContentDelta,
    },

    /// End of a content block.
    ContentBlockStop {
        /// Index of the content block that finished.
        index: usize,
    },

    /// Final metadata about the message.
    MessageDelta {
        /// The message delta with stop_reason.
        delta: MessageDelta,
        /// Final usage information.
        #[serde(skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
    },

    /// End of the message response.
    MessageStop,

    /// Keep-alive event.
    Ping,

    /// Error during streaming.
    Error {
        /// The error details.
        error: StreamError,
    },
}

impl StreamEvent {
    /// Create a message_start event.
    pub fn message_start(message: PartialMessage) -> Self {
        StreamEvent::MessageStart { message }
    }

    /// Create a content_block_start event.
    pub fn content_block_start(index: usize, content_block: ContentBlock) -> Self {
        StreamEvent::ContentBlockStart {
            index,
            content_block,
        }
    }

    /// Create a content_block_delta event.
    pub fn content_block_delta(index: usize, delta: ContentDelta) -> Self {
        StreamEvent::ContentBlockDelta { index, delta }
    }

    /// Create a content_block_stop event.
    pub fn content_block_stop(index: usize) -> Self {
        StreamEvent::ContentBlockStop { index }
    }

    /// Create a message_delta event.
    pub fn message_delta(delta: MessageDelta, usage: Option<Usage>) -> Self {
        StreamEvent::MessageDelta { delta, usage }
    }

    /// Create a message_stop event.
    pub fn message_stop() -> Self {
        StreamEvent::MessageStop
    }

    /// Create a ping event.
    pub fn ping() -> Self {
        StreamEvent::Ping
    }

    /// Create an error event.
    pub fn error(error: StreamError) -> Self {
        StreamEvent::Error { error }
    }

    /// Check if this is a message_start event.
    pub fn is_message_start(&self) -> bool {
        matches!(self, StreamEvent::MessageStart { .. })
    }

    /// Check if this is a content_block_start event.
    pub fn is_content_block_start(&self) -> bool {
        matches!(self, StreamEvent::ContentBlockStart { .. })
    }

    /// Check if this is a content_block_delta event.
    pub fn is_content_block_delta(&self) -> bool {
        matches!(self, StreamEvent::ContentBlockDelta { .. })
    }

    /// Check if this is a content_block_stop event.
    pub fn is_content_block_stop(&self) -> bool {
        matches!(self, StreamEvent::ContentBlockStop { .. })
    }

    /// Check if this is a message_delta event.
    pub fn is_message_delta(&self) -> bool {
        matches!(self, StreamEvent::MessageDelta { .. })
    }

    /// Check if this is a message_stop event.
    pub fn is_message_stop(&self) -> bool {
        matches!(self, StreamEvent::MessageStop)
    }

    /// Check if this is a ping event.
    pub fn is_ping(&self) -> bool {
        matches!(self, StreamEvent::Ping)
    }

    /// Check if this is an error event.
    pub fn is_error(&self) -> bool {
        matches!(self, StreamEvent::Error { .. })
    }

    /// Get the event type name as a string.
    pub fn event_type(&self) -> &'static str {
        match self {
            StreamEvent::MessageStart { .. } => "message_start",
            StreamEvent::ContentBlockStart { .. } => "content_block_start",
            StreamEvent::ContentBlockDelta { .. } => "content_block_delta",
            StreamEvent::ContentBlockStop { .. } => "content_block_stop",
            StreamEvent::MessageDelta { .. } => "message_delta",
            StreamEvent::MessageStop => "message_stop",
            StreamEvent::Ping => "ping",
            StreamEvent::Error { .. } => "error",
        }
    }

    /// Get the content block index if this is a content-related event.
    pub fn content_index(&self) -> Option<usize> {
        match self {
            StreamEvent::ContentBlockStart { index, .. }
            | StreamEvent::ContentBlockDelta { index, .. }
            | StreamEvent::ContentBlockStop { index } => Some(*index),
            _ => None,
        }
    }
}

/// Delta types for incremental content updates.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentDelta {
    /// Incremental text content.
    TextDelta {
        /// The text to append.
        text: String,
    },

    /// Incremental JSON for tool input.
    InputJsonDelta {
        /// The partial JSON to append.
        partial_json: String,
    },

    /// Incremental thinking content.
    ThinkingDelta {
        /// The thinking text to append.
        thinking: String,
    },

    /// Signature for thinking block.
    SignatureDelta {
        /// The signature value.
        signature: String,
    },
}

impl ContentDelta {
    /// Create a text delta.
    pub fn text(text: impl Into<String>) -> Self {
        ContentDelta::TextDelta { text: text.into() }
    }

    /// Create an input_json delta.
    pub fn input_json(partial_json: impl Into<String>) -> Self {
        ContentDelta::InputJsonDelta {
            partial_json: partial_json.into(),
        }
    }

    /// Create a thinking delta.
    pub fn thinking(thinking: impl Into<String>) -> Self {
        ContentDelta::ThinkingDelta {
            thinking: thinking.into(),
        }
    }

    /// Create a signature delta.
    pub fn signature(signature: impl Into<String>) -> Self {
        ContentDelta::SignatureDelta {
            signature: signature.into(),
        }
    }

    /// Check if this is a text delta.
    pub fn is_text(&self) -> bool {
        matches!(self, ContentDelta::TextDelta { .. })
    }

    /// Check if this is an input_json delta.
    pub fn is_input_json(&self) -> bool {
        matches!(self, ContentDelta::InputJsonDelta { .. })
    }

    /// Check if this is a thinking delta.
    pub fn is_thinking(&self) -> bool {
        matches!(self, ContentDelta::ThinkingDelta { .. })
    }

    /// Check if this is a signature delta.
    pub fn is_signature(&self) -> bool {
        matches!(self, ContentDelta::SignatureDelta { .. })
    }

    /// Get the text if this is a text delta.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            ContentDelta::TextDelta { text } => Some(text),
            _ => None,
        }
    }

    /// Get the thinking text if this is a thinking delta.
    pub fn as_thinking(&self) -> Option<&str> {
        match self {
            ContentDelta::ThinkingDelta { thinking } => Some(thinking),
            _ => None,
        }
    }

    /// Get the partial JSON if this is an input_json delta.
    pub fn as_input_json(&self) -> Option<&str> {
        match self {
            ContentDelta::InputJsonDelta { partial_json } => Some(partial_json),
            _ => None,
        }
    }
}

/// Message delta containing final metadata.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct MessageDelta {
    /// The reason generation stopped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<StopReason>,

    /// Stop sequence that caused the stop (if applicable).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_sequence: Option<String>,
}

impl MessageDelta {
    /// Create a new message delta.
    pub fn new(stop_reason: Option<StopReason>) -> Self {
        Self {
            stop_reason,
            stop_sequence: None,
        }
    }

    /// Create a message delta with a stop sequence.
    pub fn with_stop_sequence(stop_reason: StopReason, stop_sequence: impl Into<String>) -> Self {
        Self {
            stop_reason: Some(stop_reason),
            stop_sequence: Some(stop_sequence.into()),
        }
    }
}

/// Partial message sent at the start of streaming.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PartialMessage {
    /// Unique identifier for this message.
    pub id: String,

    /// The type (always "message").
    #[serde(rename = "type", default = "default_message_type")]
    pub message_type: String,

    /// The role (always "assistant").
    pub role: Role,

    /// Content array (starts empty).
    #[serde(default)]
    pub content: Vec<ContentBlock>,

    /// The model being used.
    pub model: String,

    /// Initial usage (may be partial).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

fn default_message_type() -> String {
    "message".to_string()
}

impl PartialMessage {
    /// Create a new partial message.
    pub fn new(id: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            message_type: "message".to_string(),
            role: Role::Assistant,
            content: Vec::new(),
            model: model.into(),
            usage: None,
        }
    }

    /// Create a partial message with initial usage.
    pub fn with_usage(id: impl Into<String>, model: impl Into<String>, usage: Usage) -> Self {
        Self {
            id: id.into(),
            message_type: "message".to_string(),
            role: Role::Assistant,
            content: Vec::new(),
            model: model.into(),
            usage: Some(usage),
        }
    }
}

/// Error information for stream errors.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StreamError {
    /// Error type/code.
    #[serde(rename = "type")]
    pub error_type: String,

    /// Human-readable error message.
    pub message: String,
}

impl StreamError {
    /// Create a new stream error.
    pub fn new(error_type: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            error_type: error_type.into(),
            message: message.into(),
        }
    }

    /// Create an overloaded error.
    pub fn overloaded(message: impl Into<String>) -> Self {
        Self::new("overloaded_error", message)
    }

    /// Create a rate limit error.
    pub fn rate_limit(message: impl Into<String>) -> Self {
        Self::new("rate_limit_error", message)
    }

    /// Create an API error.
    pub fn api_error(message: impl Into<String>) -> Self {
        Self::new("api_error", message)
    }

    /// Check if this is a rate limit error.
    pub fn is_rate_limit(&self) -> bool {
        self.error_type == "rate_limit_error"
    }

    /// Check if this is an overloaded error.
    pub fn is_overloaded(&self) -> bool {
        self.error_type == "overloaded_error"
    }
}

impl std::fmt::Display for StreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.error_type, self.message)
    }
}

impl std::error::Error for StreamError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stream_event_message_start() {
        let msg = PartialMessage::new("msg_123", "claude-sonnet-4-5");
        let event = StreamEvent::message_start(msg);

        assert!(event.is_message_start());
        assert_eq!(event.event_type(), "message_start");
        assert!(event.content_index().is_none());

        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "message_start");
        assert_eq!(json["message"]["id"], "msg_123");
    }

    #[test]
    fn test_stream_event_content_block_start() {
        let event = StreamEvent::content_block_start(0, ContentBlock::text(""));

        assert!(event.is_content_block_start());
        assert_eq!(event.content_index(), Some(0));

        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "content_block_start");
        assert_eq!(json["index"], 0);
    }

    #[test]
    fn test_stream_event_content_block_delta() {
        let event = StreamEvent::content_block_delta(0, ContentDelta::text("Hello"));

        assert!(event.is_content_block_delta());
        assert_eq!(event.content_index(), Some(0));

        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "content_block_delta");
        assert_eq!(json["delta"]["type"], "text_delta");
        assert_eq!(json["delta"]["text"], "Hello");
    }

    #[test]
    fn test_stream_event_content_block_stop() {
        let event = StreamEvent::content_block_stop(0);

        assert!(event.is_content_block_stop());
        assert_eq!(event.content_index(), Some(0));

        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "content_block_stop");
        assert_eq!(json["index"], 0);
    }

    #[test]
    fn test_stream_event_message_delta() {
        let delta = MessageDelta::new(Some(StopReason::EndTurn));
        let usage = Usage::new(10, 5);
        let event = StreamEvent::message_delta(delta, Some(usage));

        assert!(event.is_message_delta());

        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "message_delta");
        assert_eq!(json["delta"]["stop_reason"], "end_turn");
        assert_eq!(json["usage"]["input_tokens"], 10);
    }

    #[test]
    fn test_stream_event_message_stop() {
        let event = StreamEvent::message_stop();

        assert!(event.is_message_stop());
        assert_eq!(event.event_type(), "message_stop");
    }

    #[test]
    fn test_stream_event_ping() {
        let event = StreamEvent::ping();

        assert!(event.is_ping());
        assert_eq!(event.event_type(), "ping");

        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "ping");
    }

    #[test]
    fn test_stream_event_error() {
        let error = StreamError::rate_limit("Too many requests");
        let event = StreamEvent::error(error);

        assert!(event.is_error());

        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "error");
        assert_eq!(json["error"]["type"], "rate_limit_error");
    }

    #[test]
    fn test_content_delta_text() {
        let delta = ContentDelta::text("Hello, world!");

        assert!(delta.is_text());
        assert_eq!(delta.as_text(), Some("Hello, world!"));

        let json = serde_json::to_value(&delta).unwrap();
        assert_eq!(json["type"], "text_delta");
        assert_eq!(json["text"], "Hello, world!");
    }

    #[test]
    fn test_content_delta_input_json() {
        let delta = ContentDelta::input_json(r#"{"key": "val"#);

        assert!(delta.is_input_json());
        assert_eq!(delta.as_input_json(), Some(r#"{"key": "val"#));

        let json = serde_json::to_value(&delta).unwrap();
        assert_eq!(json["type"], "input_json_delta");
    }

    #[test]
    fn test_content_delta_thinking() {
        let delta = ContentDelta::thinking("Let me consider...");

        assert!(delta.is_thinking());
        assert_eq!(delta.as_thinking(), Some("Let me consider..."));

        let json = serde_json::to_value(&delta).unwrap();
        assert_eq!(json["type"], "thinking_delta");
    }

    #[test]
    fn test_content_delta_signature() {
        let delta = ContentDelta::signature("sig_abc123");

        assert!(delta.is_signature());

        let json = serde_json::to_value(&delta).unwrap();
        assert_eq!(json["type"], "signature_delta");
        assert_eq!(json["signature"], "sig_abc123");
    }

    #[test]
    fn test_content_delta_deserialization() {
        let text: ContentDelta =
            serde_json::from_str(r#"{"type": "text_delta", "text": "Hi"}"#).unwrap();
        assert!(text.is_text());

        let thinking: ContentDelta =
            serde_json::from_str(r#"{"type": "thinking_delta", "thinking": "Hmm"}"#).unwrap();
        assert!(thinking.is_thinking());
    }

    #[test]
    fn test_message_delta_creation() {
        let delta = MessageDelta::new(Some(StopReason::EndTurn));
        assert_eq!(delta.stop_reason, Some(StopReason::EndTurn));
        assert!(delta.stop_sequence.is_none());
    }

    #[test]
    fn test_message_delta_with_stop_sequence() {
        let delta = MessageDelta::with_stop_sequence(StopReason::StopSequence, "END");
        assert_eq!(delta.stop_reason, Some(StopReason::StopSequence));
        assert_eq!(delta.stop_sequence, Some("END".to_string()));
    }

    #[test]
    fn test_message_delta_serialization() {
        let delta = MessageDelta::new(Some(StopReason::ToolUse));
        let json = serde_json::to_value(&delta).unwrap();
        assert_eq!(json["stop_reason"], "tool_use");
    }

    #[test]
    fn test_partial_message_creation() {
        let msg = PartialMessage::new("msg_123", "claude-sonnet-4-5");

        assert_eq!(msg.id, "msg_123");
        assert_eq!(msg.model, "claude-sonnet-4-5");
        assert_eq!(msg.role, Role::Assistant);
        assert!(msg.content.is_empty());
    }

    #[test]
    fn test_partial_message_with_usage() {
        let usage = Usage::new(100, 0);
        let msg = PartialMessage::with_usage("msg_123", "claude-sonnet-4-5", usage);

        assert!(msg.usage.is_some());
        assert_eq!(msg.usage.unwrap().input_tokens, 100);
    }

    #[test]
    fn test_partial_message_serialization() {
        let msg = PartialMessage::new("msg_123", "claude-sonnet-4-5");
        let json = serde_json::to_value(&msg).unwrap();

        assert_eq!(json["id"], "msg_123");
        assert_eq!(json["type"], "message");
        assert_eq!(json["role"], "assistant");
        assert_eq!(json["model"], "claude-sonnet-4-5");
        assert!(json["content"].as_array().unwrap().is_empty());
    }

    #[test]
    fn test_stream_error_creation() {
        let error = StreamError::new("custom_error", "Something went wrong");
        assert_eq!(error.error_type, "custom_error");
        assert_eq!(error.message, "Something went wrong");
    }

    #[test]
    fn test_stream_error_types() {
        let rate_limit = StreamError::rate_limit("Rate limited");
        assert!(rate_limit.is_rate_limit());
        assert!(!rate_limit.is_overloaded());

        let overloaded = StreamError::overloaded("System busy");
        assert!(overloaded.is_overloaded());
        assert!(!overloaded.is_rate_limit());

        let api_error = StreamError::api_error("Server error");
        assert!(!api_error.is_rate_limit());
        assert!(!api_error.is_overloaded());
    }

    #[test]
    fn test_stream_error_display() {
        let error = StreamError::rate_limit("Too many requests");
        assert_eq!(error.to_string(), "rate_limit_error: Too many requests");
    }

    #[test]
    fn test_stream_error_serialization() {
        let error = StreamError::api_error("Internal error");
        let json = serde_json::to_value(&error).unwrap();

        assert_eq!(json["type"], "api_error");
        assert_eq!(json["message"], "Internal error");
    }

    #[test]
    fn test_stream_event_deserialization() {
        let json = r#"{"type": "message_start", "message": {"id": "msg_1", "type": "message", "role": "assistant", "content": [], "model": "claude-sonnet-4-5"}}"#;
        let event: StreamEvent = serde_json::from_str(json).unwrap();
        assert!(event.is_message_start());

        let ping_json = r#"{"type": "ping"}"#;
        let ping: StreamEvent = serde_json::from_str(ping_json).unwrap();
        assert!(ping.is_ping());
    }

    #[test]
    fn test_stream_event_roundtrip() {
        let events = vec![
            StreamEvent::message_start(PartialMessage::new("msg_1", "claude-sonnet-4-5")),
            StreamEvent::content_block_start(0, ContentBlock::text("")),
            StreamEvent::content_block_delta(0, ContentDelta::text("Hello")),
            StreamEvent::content_block_stop(0),
            StreamEvent::message_delta(MessageDelta::new(Some(StopReason::EndTurn)), None),
            StreamEvent::message_stop(),
            StreamEvent::ping(),
            StreamEvent::error(StreamError::api_error("test")),
        ];

        for original in events {
            let serialized = serde_json::to_string(&original).unwrap();
            let deserialized: StreamEvent = serde_json::from_str(&serialized).unwrap();
            assert_eq!(original, deserialized);
        }
    }

    #[test]
    fn test_full_streaming_sequence() {
        // Simulate a complete streaming response
        let events: [StreamEvent; 8] = [
            StreamEvent::message_start(PartialMessage::new("msg_1", "claude-sonnet-4-5")),
            StreamEvent::content_block_start(0, ContentBlock::text("")),
            StreamEvent::content_block_delta(0, ContentDelta::text("Hello")),
            StreamEvent::content_block_delta(0, ContentDelta::text(", ")),
            StreamEvent::content_block_delta(0, ContentDelta::text("world!")),
            StreamEvent::content_block_stop(0),
            StreamEvent::message_delta(
                MessageDelta::new(Some(StopReason::EndTurn)),
                Some(Usage::new(10, 3)),
            ),
            StreamEvent::message_stop(),
        ];

        // Verify correct sequence
        assert!(events[0].is_message_start());
        assert!(events[1].is_content_block_start());
        assert!(events[2].is_content_block_delta());
        assert!(events[5].is_content_block_stop());
        assert!(events[6].is_message_delta());
        assert!(events[7].is_message_stop());

        // Verify indices
        assert_eq!(events[1].content_index(), Some(0));
        assert_eq!(events[2].content_index(), Some(0));
        assert_eq!(events[5].content_index(), Some(0));
    }
}
