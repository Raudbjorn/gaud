//! Anthropic Messages API response types.

use serde::{Deserialize, Serialize};

/// A Messages API response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessagesResponse {
    /// Response ID.
    pub id: String,
    /// Object type (always "message").
    #[serde(rename = "type")]
    pub response_type: String,
    /// Role (always "assistant").
    pub role: String,
    /// Response content blocks.
    pub content: Vec<ResponseContentBlock>,
    /// Model used.
    pub model: String,
    /// Stop reason.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<StopReason>,
    /// Stop sequence that triggered stop, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_sequence: Option<String>,
    /// Token usage information.
    pub usage: Usage,
}

impl MessagesResponse {
    /// Extract all text content from the response.
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|b| match b {
                ResponseContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }

    /// Extract tool use blocks from the response.
    pub fn tool_uses(&self) -> Vec<&ResponseContentBlock> {
        self.content
            .iter()
            .filter(|b| matches!(b, ResponseContentBlock::ToolUse { .. }))
            .collect()
    }

    /// Check if the response contains tool calls.
    pub fn has_tool_use(&self) -> bool {
        self.content
            .iter()
            .any(|b| matches!(b, ResponseContentBlock::ToolUse { .. }))
    }
}

/// A content block in a response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ResponseContentBlock {
    /// Text content.
    #[serde(rename = "text")]
    Text { text: String },
    /// Tool use.
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// Thinking content.
    #[serde(rename = "thinking")]
    Thinking {
        thinking: String,
    },
}

/// Reason the model stopped generating.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// Model reached a natural end.
    EndTurn,
    /// Model wants to use a tool.
    ToolUse,
    /// Max tokens reached.
    MaxTokens,
    /// Stop sequence matched.
    StopSequence,
}

/// Token usage information.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    /// Input/prompt tokens.
    pub input_tokens: u32,
    /// Output/completion tokens.
    pub output_tokens: u32,
    /// Cache creation input tokens (if applicable).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<u32>,
    /// Cache read input tokens (if applicable).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<u32>,
}
