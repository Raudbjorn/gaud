//! Anthropic Messages API response types.
//!
//! This module provides the response types returned by the Messages API,
//! including `MessagesResponse`, `Usage`, and `StopReason`.

use serde::{Deserialize, Serialize};

use super::content::ContentBlock;
use super::request::Role;

/// Response from the Messages API.
///
/// Contains the model's generated content along with metadata about
/// token usage and the reason generation stopped.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MessagesResponse {
    /// Unique identifier for this response.
    pub id: String,

    /// The type of response (always "message" for non-streaming).
    #[serde(rename = "type", default = "default_message_type")]
    pub response_type: String,

    /// The model that generated the response.
    pub model: String,

    /// The role of the response (always "assistant").
    pub role: Role,

    /// The generated content blocks.
    pub content: Vec<ContentBlock>,

    /// The reason generation stopped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<StopReason>,

    /// Stop sequence that triggered the stop (if stop_reason is stop_sequence).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_sequence: Option<String>,

    /// Token usage information.
    pub usage: Usage,
}

fn default_message_type() -> String {
    "message".to_string()
}

impl MessagesResponse {
    /// Create a new response with the given parameters.
    pub fn new(
        id: impl Into<String>,
        model: impl Into<String>,
        content: Vec<ContentBlock>,
        stop_reason: Option<StopReason>,
        usage: Usage,
    ) -> Self {
        Self {
            id: id.into(),
            response_type: "message".to_string(),
            model: model.into(),
            role: Role::Assistant,
            content,
            stop_reason,
            stop_sequence: None,
            usage,
        }
    }

    /// Extract all text content from the response.
    ///
    /// Concatenates text from all text blocks, separated by newlines.
    /// Returns an empty string if there are no text blocks.
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|block| block.as_text())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Get all tool use blocks from the response.
    ///
    /// Returns an iterator over tool use blocks.
    pub fn tool_calls(&self) -> impl Iterator<Item = &ContentBlock> {
        self.content.iter().filter(|block| block.is_tool_use())
    }

    /// Get the first tool use block if present.
    pub fn first_tool_call(&self) -> Option<&ContentBlock> {
        self.tool_calls().next()
    }

    /// Get all thinking blocks from the response.
    ///
    /// Returns an iterator over thinking blocks.
    pub fn thinking_blocks(&self) -> impl Iterator<Item = &ContentBlock> {
        self.content.iter().filter(|block| block.is_thinking())
    }

    /// Get the thinking content if present.
    ///
    /// Concatenates all thinking blocks, separated by newlines.
    /// Returns None if there are no thinking blocks.
    pub fn thinking(&self) -> Option<String> {
        let thinking: Vec<&str> = self
            .content
            .iter()
            .filter_map(|block| block.as_thinking())
            .collect();

        if thinking.is_empty() {
            None
        } else {
            Some(thinking.join("\n"))
        }
    }

    /// Check if the response contains any tool calls.
    pub fn has_tool_calls(&self) -> bool {
        self.content.iter().any(|block| block.is_tool_use())
    }

    /// Check if the response contains thinking blocks.
    pub fn has_thinking(&self) -> bool {
        self.content.iter().any(|block| block.is_thinking())
    }

    /// Check if generation stopped due to hitting max tokens.
    pub fn is_truncated(&self) -> bool {
        matches!(self.stop_reason, Some(StopReason::MaxTokens))
    }

    /// Check if generation completed normally.
    pub fn is_complete(&self) -> bool {
        matches!(
            self.stop_reason,
            Some(StopReason::EndTurn) | Some(StopReason::StopSequence)
        )
    }

    /// Check if the model wants to use tools.
    pub fn wants_tool_use(&self) -> bool {
        matches!(self.stop_reason, Some(StopReason::ToolUse))
    }
}

/// Reason why generation stopped.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// The model finished generating naturally.
    EndTurn,

    /// Hit the maximum token limit.
    MaxTokens,

    /// Hit a stop sequence.
    StopSequence,

    /// The model wants to use a tool.
    ToolUse,
}

impl StopReason {
    /// Check if this indicates a normal completion.
    pub fn is_normal_completion(&self) -> bool {
        matches!(self, StopReason::EndTurn | StopReason::StopSequence)
    }

    /// Check if this indicates truncation.
    pub fn is_truncated(&self) -> bool {
        matches!(self, StopReason::MaxTokens)
    }

    /// Check if this indicates tool use.
    pub fn is_tool_use(&self) -> bool {
        matches!(self, StopReason::ToolUse)
    }
}

impl std::fmt::Display for StopReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StopReason::EndTurn => write!(f, "end_turn"),
            StopReason::MaxTokens => write!(f, "max_tokens"),
            StopReason::StopSequence => write!(f, "stop_sequence"),
            StopReason::ToolUse => write!(f, "tool_use"),
        }
    }
}

/// Token usage information.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Usage {
    /// Number of tokens in the input/prompt.
    pub input_tokens: u32,

    /// Number of tokens in the output/completion.
    pub output_tokens: u32,

    /// Tokens used to create cache entries (for prompt caching).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<u32>,

    /// Tokens read from cache (for prompt caching).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<u32>,
}

impl Usage {
    /// Create a new usage instance with the given input and output tokens.
    pub fn new(input_tokens: u32, output_tokens: u32) -> Self {
        Self {
            input_tokens,
            output_tokens,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        }
    }

    /// Create a usage instance with caching information.
    pub fn with_cache(
        input_tokens: u32,
        output_tokens: u32,
        cache_creation: Option<u32>,
        cache_read: Option<u32>,
    ) -> Self {
        Self {
            input_tokens,
            output_tokens,
            cache_creation_input_tokens: cache_creation,
            cache_read_input_tokens: cache_read,
        }
    }

    /// Get the total tokens used (input + output).
    pub fn total_tokens(&self) -> u32 {
        self.input_tokens + self.output_tokens
    }

    /// Check if cache was used.
    pub fn used_cache(&self) -> bool {
        self.cache_read_input_tokens.is_some_and(|t| t > 0)
    }

    /// Check if cache was created.
    pub fn created_cache(&self) -> bool {
        self.cache_creation_input_tokens.is_some_and(|t| t > 0)
    }

    /// Get the effective input tokens (excluding cache reads).
    ///
    /// This represents tokens that were actually processed, not read from cache.
    pub fn effective_input_tokens(&self) -> u32 {
        self.input_tokens - self.cache_read_input_tokens.unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_response() -> MessagesResponse {
        MessagesResponse::new(
            "msg_123",
            "claude-sonnet-4-5",
            vec![ContentBlock::text("Hello, world!")],
            Some(StopReason::EndTurn),
            Usage::new(10, 5),
        )
    }

    #[test]
    fn test_response_creation() {
        let response = sample_response();

        assert_eq!(response.id, "msg_123");
        assert_eq!(response.model, "claude-sonnet-4-5");
        assert_eq!(response.role, Role::Assistant);
        assert_eq!(response.content.len(), 1);
        assert_eq!(response.stop_reason, Some(StopReason::EndTurn));
    }

    #[test]
    fn test_response_text() {
        let response = MessagesResponse::new(
            "msg_123",
            "claude-sonnet-4-5",
            vec![ContentBlock::text("Hello"), ContentBlock::text("World")],
            Some(StopReason::EndTurn),
            Usage::new(10, 5),
        );

        assert_eq!(response.text(), "Hello\nWorld");
    }

    #[test]
    fn test_response_text_empty() {
        let response = MessagesResponse::new(
            "msg_123",
            "claude-sonnet-4-5",
            vec![],
            Some(StopReason::EndTurn),
            Usage::new(10, 0),
        );

        assert_eq!(response.text(), "");
    }

    #[test]
    fn test_response_tool_calls() {
        let response = MessagesResponse::new(
            "msg_123",
            "claude-sonnet-4-5",
            vec![
                ContentBlock::text("Let me check the weather"),
                ContentBlock::tool_use("toolu_1", "get_weather", json!({"location": "NYC"})),
                ContentBlock::tool_use("toolu_2", "get_time", json!({})),
            ],
            Some(StopReason::ToolUse),
            Usage::new(10, 20),
        );

        assert!(response.has_tool_calls());
        assert_eq!(response.tool_calls().count(), 2);
        assert!(response.first_tool_call().is_some());
        assert!(response.wants_tool_use());
    }

    #[test]
    fn test_response_thinking() {
        let response = MessagesResponse::new(
            "msg_123",
            "claude-sonnet-4-5-thinking",
            vec![
                ContentBlock::thinking("Let me analyze this...", Some("sig1".to_string())),
                ContentBlock::thinking("I should consider...", Some("sig2".to_string())),
                ContentBlock::text("Here's my answer"),
            ],
            Some(StopReason::EndTurn),
            Usage::new(10, 100),
        );

        assert!(response.has_thinking());
        assert_eq!(response.thinking_blocks().count(), 2);
        let thinking = response.thinking().unwrap();
        assert!(thinking.contains("Let me analyze"));
        assert!(thinking.contains("I should consider"));
    }

    #[test]
    fn test_response_no_thinking() {
        let response = sample_response();
        assert!(!response.has_thinking());
        assert!(response.thinking().is_none());
    }

    #[test]
    fn test_response_is_truncated() {
        let mut response = sample_response();
        response.stop_reason = Some(StopReason::MaxTokens);

        assert!(response.is_truncated());
        assert!(!response.is_complete());
    }

    #[test]
    fn test_response_is_complete() {
        let response = sample_response();
        assert!(response.is_complete());
        assert!(!response.is_truncated());
    }

    #[test]
    fn test_response_serialization() {
        let response = sample_response();
        let json = serde_json::to_value(&response).unwrap();

        assert_eq!(json["id"], "msg_123");
        assert_eq!(json["type"], "message");
        assert_eq!(json["model"], "claude-sonnet-4-5");
        assert_eq!(json["role"], "assistant");
        assert_eq!(json["stop_reason"], "end_turn");
        assert!(json["content"].is_array());
        assert!(json["usage"].is_object());
    }

    #[test]
    fn test_response_deserialization() {
        let json = r#"{
            "id": "msg_abc",
            "type": "message",
            "model": "claude-sonnet-4-5",
            "role": "assistant",
            "content": [{"type": "text", "text": "Hello!"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 10, "output_tokens": 5}
        }"#;

        let response: MessagesResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.id, "msg_abc");
        assert_eq!(response.model, "claude-sonnet-4-5");
        assert_eq!(response.text(), "Hello!");
    }

    #[test]
    fn test_response_roundtrip() {
        let original = sample_response();
        let serialized = serde_json::to_string(&original).unwrap();
        let deserialized: MessagesResponse = serde_json::from_str(&serialized).unwrap();

        assert_eq!(original, deserialized);
    }

    #[test]
    fn test_stop_reason_serialization() {
        assert_eq!(
            serde_json::to_string(&StopReason::EndTurn).unwrap(),
            r#""end_turn""#
        );
        assert_eq!(
            serde_json::to_string(&StopReason::MaxTokens).unwrap(),
            r#""max_tokens""#
        );
        assert_eq!(
            serde_json::to_string(&StopReason::StopSequence).unwrap(),
            r#""stop_sequence""#
        );
        assert_eq!(
            serde_json::to_string(&StopReason::ToolUse).unwrap(),
            r#""tool_use""#
        );
    }

    #[test]
    fn test_stop_reason_deserialization() {
        let end_turn: StopReason = serde_json::from_str(r#""end_turn""#).unwrap();
        assert_eq!(end_turn, StopReason::EndTurn);

        let max_tokens: StopReason = serde_json::from_str(r#""max_tokens""#).unwrap();
        assert_eq!(max_tokens, StopReason::MaxTokens);
    }

    #[test]
    fn test_stop_reason_helpers() {
        assert!(StopReason::EndTurn.is_normal_completion());
        assert!(StopReason::StopSequence.is_normal_completion());
        assert!(!StopReason::MaxTokens.is_normal_completion());
        assert!(!StopReason::ToolUse.is_normal_completion());

        assert!(StopReason::MaxTokens.is_truncated());
        assert!(!StopReason::EndTurn.is_truncated());

        assert!(StopReason::ToolUse.is_tool_use());
        assert!(!StopReason::EndTurn.is_tool_use());
    }

    #[test]
    fn test_stop_reason_display() {
        assert_eq!(StopReason::EndTurn.to_string(), "end_turn");
        assert_eq!(StopReason::MaxTokens.to_string(), "max_tokens");
        assert_eq!(StopReason::StopSequence.to_string(), "stop_sequence");
        assert_eq!(StopReason::ToolUse.to_string(), "tool_use");
    }

    #[test]
    fn test_usage_creation() {
        let usage = Usage::new(100, 50);
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 50);
        assert_eq!(usage.total_tokens(), 150);
        assert!(!usage.used_cache());
        assert!(!usage.created_cache());
    }

    #[test]
    fn test_usage_with_cache() {
        let usage = Usage::with_cache(100, 50, Some(80), Some(20));
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 50);
        assert_eq!(usage.cache_creation_input_tokens, Some(80));
        assert_eq!(usage.cache_read_input_tokens, Some(20));
        assert!(usage.used_cache());
        assert!(usage.created_cache());
        assert_eq!(usage.effective_input_tokens(), 80);
    }

    #[test]
    fn test_usage_cache_zero() {
        let usage = Usage::with_cache(100, 50, Some(0), Some(0));
        assert!(!usage.used_cache());
        assert!(!usage.created_cache());
    }

    #[test]
    fn test_usage_serialization() {
        let usage = Usage::new(100, 50);
        let json = serde_json::to_value(&usage).unwrap();

        assert_eq!(json["input_tokens"], 100);
        assert_eq!(json["output_tokens"], 50);
        // Optional fields should not appear when None
        assert!(json.get("cache_creation_input_tokens").is_none());
        assert!(json.get("cache_read_input_tokens").is_none());
    }

    #[test]
    fn test_usage_serialization_with_cache() {
        let usage = Usage::with_cache(100, 50, Some(80), Some(20));
        let json = serde_json::to_value(&usage).unwrap();

        assert_eq!(json["cache_creation_input_tokens"], 80);
        assert_eq!(json["cache_read_input_tokens"], 20);
    }

    #[test]
    fn test_usage_deserialization() {
        let json = r#"{"input_tokens": 100, "output_tokens": 50}"#;
        let usage: Usage = serde_json::from_str(json).unwrap();

        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 50);
        assert!(usage.cache_creation_input_tokens.is_none());
    }

    #[test]
    fn test_usage_deserialization_with_cache() {
        let json = r#"{
            "input_tokens": 100,
            "output_tokens": 50,
            "cache_creation_input_tokens": 80,
            "cache_read_input_tokens": 20
        }"#;
        let usage: Usage = serde_json::from_str(json).unwrap();

        assert_eq!(usage.cache_creation_input_tokens, Some(80));
        assert_eq!(usage.cache_read_input_tokens, Some(20));
    }

    #[test]
    fn test_usage_roundtrip() {
        let original = Usage::with_cache(100, 50, Some(80), Some(20));
        let serialized = serde_json::to_string(&original).unwrap();
        let deserialized: Usage = serde_json::from_str(&serialized).unwrap();

        assert_eq!(original, deserialized);
    }

    #[test]
    fn test_usage_default() {
        let usage = Usage::default();
        assert_eq!(usage.input_tokens, 0);
        assert_eq!(usage.output_tokens, 0);
        assert!(usage.cache_creation_input_tokens.is_none());
        assert!(usage.cache_read_input_tokens.is_none());
    }
}
