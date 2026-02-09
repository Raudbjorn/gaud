//! Streaming event types for the Messages API.

use serde::{Deserialize, Serialize};

use super::response::{ResponseContentBlock, StopReason, Usage};

/// A streaming event from the Messages API.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum StreamEvent {
    /// Stream started.
    #[serde(rename = "message_start")]
    MessageStart {
        message: PartialMessage,
    },
    /// New content block started.
    #[serde(rename = "content_block_start")]
    ContentBlockStart {
        index: usize,
        content_block: ResponseContentBlock,
    },
    /// Delta within a content block.
    #[serde(rename = "content_block_delta")]
    ContentBlockDelta {
        index: usize,
        delta: ContentDelta,
    },
    /// Content block ended.
    #[serde(rename = "content_block_stop")]
    ContentBlockStop {
        index: usize,
    },
    /// Message delta (stop reason, usage).
    #[serde(rename = "message_delta")]
    MessageDelta {
        delta: MessageDelta,
        #[serde(skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
    },
    /// Stream ended.
    #[serde(rename = "message_stop")]
    MessageStop,

    /// Ping (keepalive).
    #[serde(rename = "ping")]
    Ping,

    /// Error event.
    #[serde(rename = "error")]
    Error {
        error: StreamError,
    },
}

/// Partial message at stream start.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartialMessage {
    pub id: String,
    #[serde(rename = "type")]
    pub message_type: String,
    pub role: String,
    pub model: String,
    pub usage: Usage,
}

/// Content delta within a content block.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentDelta {
    /// Text delta.
    #[serde(rename = "text_delta")]
    TextDelta { text: String },
    /// Tool input delta (partial JSON).
    #[serde(rename = "input_json_delta")]
    InputJsonDelta { partial_json: String },
    /// Thinking delta.
    #[serde(rename = "thinking_delta")]
    ThinkingDelta { thinking: String },
}

/// Message-level delta.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<StopReason>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_sequence: Option<String>,
}

/// Error in stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamError {
    #[serde(rename = "type")]
    pub error_type: String,
    pub message: String,
}
