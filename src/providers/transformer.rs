//! Provider transformation trait and utilities.
//!
//! Defines the core transformation interface that all providers must implement
//! for converting between OpenAI format and provider-specific formats.
//!
//! Utility functions have been migrated to `transform::util` -- this module
//! re-exports them for backward compatibility.

use std::time::Duration;

use super::types::*;
use super::ProviderError;

// Re-export utilities from the new location for backward compatibility.
pub use super::transform::util::{
    convert_anthropic_tool_use_to_openai, convert_tool_choice, convert_tools_to_anthropic,
    extract_system_message, filter_system_messages, normalize_stop_sequences, parse_image_url,
};

// MARK: - Provider Response Metadata

/// Metadata extracted from a provider's HTTP response.
///
/// Carries rate limit headers, retry-after information, and the upstream
/// status code for proper error forwarding.
#[derive(Debug, Clone, Default)]
pub struct ProviderResponseMeta {
    /// Provider identifier (e.g. "claude", "gemini").
    pub provider: String,
    /// Model identifier used for this request.
    pub model: String,
    /// Unix timestamp for the response.
    pub created: i64,
    /// Parsed retry-after duration from rate limit headers.
    pub retry_after: Option<Duration>,
    /// Normalized rate limit headers to forward to the client.
    pub rate_limit_headers: Vec<(String, String)>,
    /// Upstream HTTP status code.
    pub status_code: Option<u16>,
}

// MARK: - Stream State Trait

/// Stateful per-stream processor for SSE events.
///
/// Each streaming request creates a fresh `StreamState` instance that tracks
/// mutable state across the lifetime of one SSE stream: tool call indices,
/// accumulated arguments, token counts, response ID, etc.
///
/// This replaces the stateless `transform_stream_chunk()` method, enabling
/// correct tool call index tracking (the critical fix for REQ-TOOL-04/05/06).
pub trait StreamState: Send {
    /// Process a single SSE data payload and return an optional OpenAI chunk.
    ///
    /// The `data` parameter is the raw JSON string from the SSE `data:` line
    /// (already stripped of the `data: ` prefix by `SseParser`).
    fn process_event(&mut self, data: &str) -> Result<Option<ChatChunk>, ProviderError>;

    /// Process a pre-parsed JSON value and return an optional OpenAI chunk.
    ///
    /// Override this in providers that already have a typed/parsed event to
    /// avoid a redundant `Value → String → Value` round-trip.  The default
    /// implementation serializes the value to a string and delegates to
    /// [`process_event`](Self::process_event).
    fn process_event_value(
        &mut self,
        value: &serde_json::Value,
    ) -> Result<Option<ChatChunk>, ProviderError> {
        let s = serde_json::to_string(value)
            .map_err(|e| ProviderError::Other(format!("JSON serialization error: {e}")))?;
        self.process_event(&s)
    }

    /// Return accumulated token usage at the end of stream.
    fn final_usage(&self) -> Usage;

    /// Return the response ID assigned during streaming.
    fn response_id(&self) -> &str;
}

// MARK: - Provider Transformer Trait

/// Trait for transforming requests/responses between OpenAI format and provider-specific formats.
///
/// Inspired by litellm's BaseConfig pattern. Each provider implements this
/// trait to handle format conversion. The actual HTTP transport remains in the
/// provider's `LlmProvider` implementation.
pub trait ProviderTransformer: Send + Sync {
    /// Transform an OpenAI-format request into the provider's native format.
    fn transform_request(&self, request: &ChatRequest) -> Result<serde_json::Value, ProviderError>;

    /// Transform a provider's response into OpenAI format.
    fn transform_response(
        &self,
        response: serde_json::Value,
        meta: &ProviderResponseMeta,
    ) -> Result<ChatResponse, ProviderError>;

    /// Create a new stateful stream processor for one streaming request.
    fn new_stream_state(&self, model: &str) -> Box<dyn StreamState>;

    /// Get the provider's identifier.
    fn provider_id(&self) -> &str;

    /// Get the provider's display name.
    fn provider_name(&self) -> &str;

    /// Check if the provider supports a specific model.
    fn supports_model(&self, model: &str) -> bool;

    /// Get the list of supported models.
    fn supported_models(&self) -> Vec<String>;

    /// Get default max_tokens for the provider (if required).
    fn default_max_tokens(&self) -> Option<u32> {
        None
    }

    /// Map provider-specific finish_reason to OpenAI format.
    fn map_finish_reason(&self, reason: &str) -> &'static str {
        super::transform::util::map_finish_reason_to_openai(reason)
    }
}

// MARK: - Tests

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_tool_choice_string() {
        let auto = Some(serde_json::json!("auto"));
        let result = convert_tool_choice(&auto, None);
        assert_eq!(result, Some(serde_json::json!({"type": "auto"})));

        let required = Some(serde_json::json!("required"));
        let result = convert_tool_choice(&required, None);
        assert_eq!(result, Some(serde_json::json!({"type": "any"})));

        let none = Some(serde_json::json!("none"));
        let result = convert_tool_choice(&none, None);
        assert_eq!(result, Some(serde_json::json!({"type": "none"})));
    }

    #[test]
    fn test_convert_tool_choice_with_function() {
        let choice = Some(serde_json::json!({
            "type": "function",
            "function": {"name": "get_weather"}
        }));
        let result = convert_tool_choice(&choice, None);
        assert_eq!(
            result,
            Some(serde_json::json!({
                "type": "tool",
                "name": "get_weather"
            }))
        );
    }

    #[test]
    fn test_convert_tool_choice_with_parallel_control() {
        let choice = Some(serde_json::json!("auto"));
        let result = convert_tool_choice(&choice, Some(false));
        assert_eq!(result, Some(serde_json::json!({"type": "auto"})));
    }

    #[test]
    fn test_extract_system_message() {
        let messages = vec![
            ChatMessage {
                role: MessageRole::System,
                content: Some(MessageContent::Text("You are helpful.".to_string())),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            },
            ChatMessage {
                role: MessageRole::User,
                content: Some(MessageContent::Text("Hello".to_string())),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            },
        ];

        let system = extract_system_message(&messages);
        assert_eq!(system, Some("You are helpful.".to_string()));
    }

    #[test]
    fn test_filter_system_messages() {
        let messages = vec![
            ChatMessage {
                role: MessageRole::System,
                content: Some(MessageContent::Text("System".to_string())),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            },
            ChatMessage {
                role: MessageRole::User,
                content: Some(MessageContent::Text("User".to_string())),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            },
        ];

        let filtered = filter_system_messages(&messages);
        assert_eq!(filtered.len(), 1);
        assert!(matches!(filtered[0].role, MessageRole::User));
    }

    #[test]
    fn test_parse_image_url_data_uri() {
        let url = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAUA";
        let (source_type, media_type, data) = parse_image_url(url);
        assert_eq!(source_type, "base64");
        assert_eq!(media_type, "image/png");
        assert_eq!(data, "iVBORw0KGgoAAAANSUhEUgAAAAUA");
    }

    #[test]
    fn test_parse_image_url_external() {
        let url = "https://example.com/image.png";
        let (source_type, media_type, data) = parse_image_url(url);
        assert_eq!(source_type, "url");
        assert_eq!(media_type, "image/png");
        assert_eq!(data, url);
    }

    #[test]
    fn test_normalize_stop_sequences() {
        let single = Some(StopSequence::Single("STOP".to_string()));
        let result = normalize_stop_sequences(&single);
        assert_eq!(result, Some(vec!["STOP".to_string()]));

        let multiple = Some(StopSequence::Multiple(vec![
            "END".to_string(),
            "STOP".to_string(),
        ]));
        let result = normalize_stop_sequences(&multiple);
        assert_eq!(result, Some(vec!["END".to_string(), "STOP".to_string()]));
    }

    #[test]
    fn test_provider_response_meta_default() {
        let meta = ProviderResponseMeta::default();
        assert!(meta.provider.is_empty());
        assert!(meta.retry_after.is_none());
        assert!(meta.rate_limit_headers.is_empty());
        assert!(meta.status_code.is_none());
    }
}
