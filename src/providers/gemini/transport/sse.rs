//! Server-Sent Events (SSE) stream parser.
//!
//! This module provides SSE parsing for streaming responses from the Cloud Code API.
//! It converts Google's streaming format into Anthropic `StreamEvent` types.
//!
//! ## SSE Format
//!
//! Cloud Code returns SSE in the format:
//! ```text
//! data: {"candidates":[...], "usageMetadata": {...}}
//!
//! data: {"candidates":[...]}
//!
//! data: [DONE]
//! ```
//!
//! ## Stream Events
//!
//! The parser emits Anthropic-format `StreamEvent`s:
//! - `message_start` - Beginning of response
//! - `content_block_start` - Start of a content block (text, thinking, tool_use)
//! - `content_block_delta` - Incremental content update
//! - `content_block_stop` - End of a content block
//! - `message_delta` - Final metadata (stop_reason, usage)
//! - `message_stop` - End of response

use std::collections::VecDeque;
use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use futures::stream::Stream;

use pin_project_lite::pin_project;
use serde::Deserialize;
use tracing::{debug, warn};

use crate::providers::gemini::constants::{MIN_SIGNATURE_LENGTH, ModelFamily, get_model_family};
use crate::providers::gemini::error::{Error, Result};
use crate::providers::gemini::models::content::ContentBlock;
use crate::providers::gemini::models::response::{StopReason, Usage};
use crate::providers::gemini::models::stream::{
    ContentDelta, MessageDelta, PartialMessage, StreamError, StreamEvent,
};
use crate::providers::gemini::thinking::GLOBAL_SIGNATURE_CACHE;

pin_project! {
    /// SSE stream parser that converts Cloud Code responses to Anthropic events.
    pub struct SseStream<S> {
        #[pin]
        byte_stream: S,
        state: StreamState,
        buffer: String,
        pending_events: VecDeque<StreamEvent>,
    }
}

impl<S> SseStream<S>
where
    S: Stream<Item = std::result::Result<Bytes, reqwest::Error>> + Send + 'static,
{
    /// Create a new SSE stream parser.
    pub fn new(byte_stream: S, model: impl Into<String>) -> Self {
        Self {
            byte_stream,
            state: StreamState::new(model.into()),
            buffer: String::new(),
            pending_events: VecDeque::new(),
        }
    }
}

impl<S> Stream for SseStream<S>
where
    S: Stream<Item = std::result::Result<Bytes, reqwest::Error>> + Send + 'static,
{
    type Item = Result<StreamEvent>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();

        // 1. Drain pending events first
        if let Some(event) = this.pending_events.pop_front() {
            return Poll::Ready(Some(Ok(event)));
        }

        // 2. Poll underlying stream
        loop {
            match this.byte_stream.as_mut().poll_next(cx) {
                Poll::Ready(Some(Ok(chunk))) => {
                    // Process chunk
                    let text = String::from_utf8_lossy(&chunk);
                    this.buffer.push_str(&text);

                    // Process complete lines
                    while let Some(newline_pos) = this.buffer.find('\n') {
                        let line = this.buffer[..newline_pos].to_string();
                        *this.buffer = this.buffer[newline_pos + 1..].to_string();

                        match process_sse_line(&line, this.state) {
                            Ok(events) => {
                                this.pending_events.extend(events);
                            }
                            Err(e) => return Poll::Ready(Some(Err(e))),
                        }
                    }

                    // If we have events now, return the first one
                    if let Some(event) = this.pending_events.pop_front() {
                        return Poll::Ready(Some(Ok(event)));
                    }
                    // Otherwise continue polling
                }
                Poll::Ready(Some(Err(e))) => return Poll::Ready(Some(Err(Error::from(e)))),
                Poll::Ready(None) => {
                    // Stream finished
                    // Process remaining buffer
                    if !this.buffer.is_empty() {
                        let line = std::mem::take(this.buffer);
                        match process_sse_line(&line, this.state) {
                            Ok(events) => this.pending_events.extend(events),
                            Err(e) => return Poll::Ready(Some(Err(e))),
                        }
                    }

                    // Finalize
                    match finalize_stream(this.state) {
                        Ok(events) => this.pending_events.extend(events),
                        Err(e) => return Poll::Ready(Some(Err(e))),
                    }

                    if let Some(event) = this.pending_events.pop_front() {
                        return Poll::Ready(Some(Ok(event)));
                    } else {
                        return Poll::Ready(None);
                    }
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

/// Internal state for stream parsing.
struct StreamState {
    /// Message ID for this response.
    message_id: String,
    /// Whether message_start has been emitted.
    has_emitted_start: bool,
    /// Current block index.
    block_index: usize,
    /// Current block type being accumulated.
    current_block_type: Option<BlockType>,
    /// Accumulated thinking signature for the current block.
    current_thinking_signature: String,
    /// Input tokens from usage metadata.
    input_tokens: u32,
    /// Output tokens from usage metadata.
    output_tokens: u32,
    /// Cache read tokens.
    cache_read_tokens: u32,
    /// Stop reason from the response.
    stop_reason: Option<StopReason>,
    /// Model name for signature caching.
    model: String,
    /// Model family.
    model_family: ModelFamily,
}

impl StreamState {
    fn new(model: String) -> Self {
        let model_family = get_model_family(&model);
        Self {
            message_id: generate_message_id(),
            has_emitted_start: false,
            block_index: 0,
            current_block_type: None,
            current_thinking_signature: String::new(),
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            stop_reason: None,
            model,
            model_family,
        }
    }
}
/// Type of content block being streamed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockType {
    Text,
    Thinking,
    ToolUse,
}

/// Process a single SSE line.
fn process_sse_line(line: &str, state: &mut StreamState) -> Result<Vec<StreamEvent>> {
    let line = line.trim();

    // Skip empty lines and comments
    if line.is_empty() || line.starts_with(':') {
        return Ok(vec![]);
    }

    // Parse data lines
    if !line.starts_with("data:") {
        return Ok(vec![]);
    }

    let json_text = line[5..].trim();

    // Handle [DONE] signal
    if json_text == "[DONE]" {
        return Ok(vec![]);
    }

    // Skip empty data
    if json_text.is_empty() {
        return Ok(vec![]);
    }

    // Parse JSON
    let data: SseData = match serde_json::from_str(json_text) {
        Ok(d) => d,
        Err(e) => {
            debug!(error = %e, data = %json_text.chars().take(100).collect::<String>(), "SSE parse warning");
            return Ok(vec![]);
        }
    };

    process_sse_data(data, state)
}

/// Process parsed SSE data.
fn process_sse_data(data: SseData, state: &mut StreamState) -> Result<Vec<StreamEvent>> {
    let mut events = Vec::new();

    // Get the inner response (may be wrapped)
    let inner = data.response.as_deref().unwrap_or(&data);

    // Update usage metadata
    if let Some(usage) = &inner.usage_metadata {
        state.input_tokens = usage.prompt_token_count;
        state.output_tokens = usage.candidates_token_count;
        state.cache_read_tokens = usage.cached_content_token_count.unwrap_or(0);
    }

    // Get parts from first candidate
    let parts = inner
        .candidates
        .as_ref()
        .and_then(|c| c.first())
        .and_then(|c| c.content.as_ref())
        .map(|c| c.parts.as_slice())
        .unwrap_or(&[]);

    // Check finish reason
    if let Some(finish_reason) = inner
        .candidates
        .as_ref()
        .and_then(|c| c.first())
        .and_then(|c| c.finish_reason.as_deref())
    {
        if state.stop_reason.is_none() {
            state.stop_reason = Some(map_finish_reason(finish_reason));
        }
    }

    // Emit message_start if not yet emitted and we have parts
    if !state.has_emitted_start && !parts.is_empty() {
        state.has_emitted_start = true;
        events.push(emit_message_start(state));
    }

    // Process each part
    for part in parts {
        let part_events = process_part(part, state)?;
        events.extend(part_events);
    }

    Ok(events)
}

/// Process a single part from the response.
fn process_part(part: &SsePart, state: &mut StreamState) -> Result<Vec<StreamEvent>> {
    let mut events = Vec::new();

    if part.thought == Some(true) {
        // Thinking part
        let text = part.text.as_deref().unwrap_or("");
        let signature = part.thought_signature.as_deref().unwrap_or("");

        // Switch to thinking block if needed
        if state.current_block_type != Some(BlockType::Thinking) {
            events.extend(close_current_block(state));
            state.current_block_type = Some(BlockType::Thinking);
            state.current_thinking_signature.clear();
            events.push(StreamEvent::content_block_start(
                state.block_index,
                ContentBlock::thinking("", None),
            ));
        }

        // Cache signature if valid
        if signature.len() >= MIN_SIGNATURE_LENGTH {
            state.current_thinking_signature = signature.to_string();
            GLOBAL_SIGNATURE_CACHE.store_thinking_signature(text, signature, state.model_family);
        }

        // Emit thinking delta
        if !text.is_empty() {
            events.push(StreamEvent::content_block_delta(
                state.block_index,
                ContentDelta::thinking(text),
            ));
        }
    } else if let Some(function_call) = &part.function_call {
        // Function call part
        events.extend(close_current_block_with_signature(state));
        state.current_block_type = Some(BlockType::ToolUse);
        state.stop_reason = Some(StopReason::ToolUse);

        // Generate tool ID if not provided
        let tool_id = function_call
            .id
            .clone()
            .unwrap_or_else(|| format!("toolu_{}", uuid::Uuid::new_v4().simple()));

        // Cache signature if present
        if let Some(sig) = &part.thought_signature {
            if sig.len() >= MIN_SIGNATURE_LENGTH {
                GLOBAL_SIGNATURE_CACHE.store_tool_signature(&tool_id, sig, state.model_family);
            }
        }

        // Emit tool_use start
        events.push(StreamEvent::content_block_start(
            state.block_index,
            ContentBlock::tool_use(&tool_id, &function_call.name, serde_json::json!({})),
        ));

        // Emit input_json delta
        let args_json =
            serde_json::to_string(&function_call.args).unwrap_or_else(|_| "{}".to_string());
        events.push(StreamEvent::content_block_delta(
            state.block_index,
            ContentDelta::input_json(args_json),
        ));
    } else if let Some(text) = &part.text {
        // Regular text part
        if text.is_empty() {
            return Ok(events);
        }

        // Switch to text block if needed
        if state.current_block_type != Some(BlockType::Text) {
            events.extend(close_current_block_with_signature(state));
            state.current_block_type = Some(BlockType::Text);
            events.push(StreamEvent::content_block_start(
                state.block_index,
                ContentBlock::text(""),
            ));
        }

        // Emit text delta
        events.push(StreamEvent::content_block_delta(
            state.block_index,
            ContentDelta::text(text),
        ));
    }

    Ok(events)
}

/// Close the current block, emitting signature if in thinking mode.
fn close_current_block_with_signature(state: &mut StreamState) -> Vec<StreamEvent> {
    let mut events = Vec::new();

    if let Some(block_type) = state.current_block_type {
        // Emit signature delta for thinking blocks
        if block_type == BlockType::Thinking && !state.current_thinking_signature.is_empty() {
            events.push(StreamEvent::content_block_delta(
                state.block_index,
                ContentDelta::signature(&state.current_thinking_signature),
            ));
            state.current_thinking_signature.clear();
        }

        events.push(StreamEvent::content_block_stop(state.block_index));
        state.block_index += 1;
        state.current_block_type = None;
    }

    events
}

/// Close the current block without emitting signature.
fn close_current_block(state: &mut StreamState) -> Vec<StreamEvent> {
    let mut events = Vec::new();

    if state.current_block_type.is_some() {
        events.push(StreamEvent::content_block_stop(state.block_index));
        state.block_index += 1;
        state.current_block_type = None;
    }

    events
}

/// Finalize the stream, emitting message_delta and message_stop.
fn finalize_stream(state: &mut StreamState) -> Result<Vec<StreamEvent>> {
    let mut events = Vec::new();

    // Close any open block
    events.extend(close_current_block_with_signature(state));

    // Emit message_delta if we emitted start
    if state.has_emitted_start {
        let usage = Usage {
            input_tokens: state.input_tokens.saturating_sub(state.cache_read_tokens),
            output_tokens: state.output_tokens,
            cache_read_input_tokens: if state.cache_read_tokens > 0 {
                Some(state.cache_read_tokens)
            } else {
                None
            },
            cache_creation_input_tokens: None,
        };

        events.push(StreamEvent::message_delta(
            MessageDelta::new(Some(state.stop_reason.unwrap_or(StopReason::EndTurn))),
            Some(usage),
        ));

        events.push(StreamEvent::message_stop());
    } else {
        // No content received - this is an error condition
        warn!("No content parts received from stream");
        events.push(StreamEvent::error(StreamError::api_error(
            "No content parts received from API",
        )));
    }

    Ok(events)
}

/// Emit the message_start event.
fn emit_message_start(state: &StreamState) -> StreamEvent {
    let usage = Usage {
        input_tokens: state.input_tokens.saturating_sub(state.cache_read_tokens),
        output_tokens: 0,
        cache_read_input_tokens: if state.cache_read_tokens > 0 {
            Some(state.cache_read_tokens)
        } else {
            None
        },
        cache_creation_input_tokens: None,
    };

    StreamEvent::message_start(PartialMessage::with_usage(
        &state.message_id,
        &state.model,
        usage,
    ))
}

/// Map Google finish reason to Anthropic stop reason.
fn map_finish_reason(reason: &str) -> StopReason {
    match reason {
        "STOP" => StopReason::EndTurn,
        "MAX_TOKENS" => StopReason::MaxTokens,
        "TOOL_USE" => StopReason::ToolUse,
        "SAFETY" => StopReason::EndTurn,
        "RECITATION" => StopReason::EndTurn,
        _ => StopReason::EndTurn,
    }
}

/// Generate a unique message ID.
fn generate_message_id() -> String {
    format!("msg_{}", uuid::Uuid::new_v4().simple())
}

// ============================================================================
// SSE Data Structures
// ============================================================================

/// Top-level SSE data structure.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SseData {
    /// Nested response (Cloud Code wraps in this).
    #[serde(default)]
    response: Option<Box<SseData>>,
    /// Candidates array.
    #[serde(default)]
    candidates: Option<Vec<SseCandidate>>,
    /// Usage metadata.
    #[serde(default)]
    usage_metadata: Option<SseUsageMetadata>,
}

/// Candidate in SSE response.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SseCandidate {
    /// Content with parts.
    #[serde(default)]
    content: Option<SseContent>,
    /// Finish reason.
    #[serde(default)]
    finish_reason: Option<String>,
}

/// Content in SSE response.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SseContent {
    /// Parts array.
    #[serde(default)]
    parts: Vec<SsePart>,
}

/// Part in SSE response.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SsePart {
    /// Text content.
    #[serde(default)]
    text: Option<String>,
    /// Whether this is a thought/thinking part.
    #[serde(default)]
    thought: Option<bool>,
    /// Thought signature.
    #[serde(default)]
    thought_signature: Option<String>,
    /// Function call.
    #[serde(default)]
    function_call: Option<SseFunctionCall>,
}

/// Function call in SSE response.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SseFunctionCall {
    /// Function name.
    name: String,
    /// Arguments.
    #[serde(default)]
    args: serde_json::Value,
    /// Optional ID.
    #[serde(default)]
    id: Option<String>,
}

/// Usage metadata in SSE response.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SseUsageMetadata {
    /// Prompt token count.
    #[serde(default)]
    prompt_token_count: u32,
    /// Candidates token count.
    #[serde(default)]
    candidates_token_count: u32,
    /// Cached content token count.
    #[serde(default)]
    cached_content_token_count: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_message_id() {
        let id1 = generate_message_id();
        let id2 = generate_message_id();

        assert!(id1.starts_with("msg_"));
        assert!(id2.starts_with("msg_"));
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_map_finish_reason() {
        assert_eq!(map_finish_reason("STOP"), StopReason::EndTurn);
        assert_eq!(map_finish_reason("MAX_TOKENS"), StopReason::MaxTokens);
        assert_eq!(map_finish_reason("TOOL_USE"), StopReason::ToolUse);
        assert_eq!(map_finish_reason("SAFETY"), StopReason::EndTurn);
        assert_eq!(map_finish_reason("UNKNOWN"), StopReason::EndTurn);
    }

    #[test]
    fn test_process_sse_line_empty() {
        let mut state = StreamState::new("claude-sonnet-4-5".to_string());

        // Empty line
        let events = process_sse_line("", &mut state).unwrap();
        assert!(events.is_empty());

        // Comment line
        let events = process_sse_line(": this is a comment", &mut state).unwrap();
        assert!(events.is_empty());

        // Non-data line
        let events = process_sse_line("event: message", &mut state).unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn test_process_sse_line_done() {
        let mut state = StreamState::new("claude-sonnet-4-5".to_string());

        let events = process_sse_line("data: [DONE]", &mut state).unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn test_process_sse_line_text() {
        let mut state = StreamState::new("claude-sonnet-4-5".to_string());
        state.has_emitted_start = true; // Skip message_start

        let json = r#"data: {"candidates":[{"content":{"parts":[{"text":"Hello"}]}}]}"#;
        let events = process_sse_line(json, &mut state).unwrap();

        assert!(!events.is_empty());
        // Should have content_block_start and content_block_delta
        let has_start = events.iter().any(|e| e.is_content_block_start());
        let has_delta = events.iter().any(|e| e.is_content_block_delta());
        assert!(has_start);
        assert!(has_delta);
    }

    #[test]
    fn test_process_sse_line_thinking() {
        let mut state = StreamState::new("gemini-3-flash".to_string());
        state.has_emitted_start = true;

        let json = r#"data: {"candidates":[{"content":{"parts":[{"thought":true,"text":"Let me think..."}]}}]}"#;
        let events = process_sse_line(json, &mut state).unwrap();

        assert!(!events.is_empty());
        assert_eq!(state.current_block_type, Some(BlockType::Thinking));
    }

    #[test]
    fn test_process_sse_line_function_call() {
        let mut state = StreamState::new("claude-sonnet-4-5".to_string());
        state.has_emitted_start = true;

        let json = r#"data: {"candidates":[{"content":{"parts":[{"functionCall":{"name":"get_weather","args":{"location":"NYC"}}}]}}]}"#;
        let events = process_sse_line(json, &mut state).unwrap();

        assert!(!events.is_empty());
        assert_eq!(state.current_block_type, Some(BlockType::ToolUse));
        assert_eq!(state.stop_reason, Some(StopReason::ToolUse));
    }

    #[test]
    fn test_process_sse_line_with_usage() {
        let mut state = StreamState::new("claude-sonnet-4-5".to_string());

        let json = r#"data: {"candidates":[{"content":{"parts":[{"text":"Hi"}]}}],"usageMetadata":{"promptTokenCount":100,"candidatesTokenCount":50,"cachedContentTokenCount":20}}"#;
        let _events = process_sse_line(json, &mut state).unwrap();

        assert_eq!(state.input_tokens, 100);
        assert_eq!(state.output_tokens, 50);
        assert_eq!(state.cache_read_tokens, 20);
    }

    #[test]
    fn test_process_sse_line_invalid_json() {
        let mut state = StreamState::new("claude-sonnet-4-5".to_string());

        let json = r#"data: {not valid json}"#;
        let events = process_sse_line(json, &mut state).unwrap();

        // Should not crash, just return empty
        assert!(events.is_empty());
    }

    #[test]
    fn test_finalize_stream_with_content() {
        let mut state = StreamState::new("claude-sonnet-4-5".to_string());
        state.has_emitted_start = true;
        state.input_tokens = 100;
        state.output_tokens = 50;

        let events = finalize_stream(&mut state).unwrap();

        // Should have message_delta and message_stop
        let has_delta = events.iter().any(|e| e.is_message_delta());
        let has_stop = events.iter().any(|e| e.is_message_stop());
        assert!(has_delta);
        assert!(has_stop);
    }

    #[test]
    fn test_finalize_stream_no_content() {
        let mut state = StreamState::new("claude-sonnet-4-5".to_string());
        // has_emitted_start is false

        let events = finalize_stream(&mut state).unwrap();

        // Should have an error event
        let has_error = events.iter().any(|e| e.is_error());
        assert!(has_error);
    }

    #[test]
    fn test_close_current_block_with_signature() {
        let mut state = StreamState::new("gemini-3-flash".to_string());
        state.current_block_type = Some(BlockType::Thinking);
        state.current_thinking_signature = "sig_".repeat(20); // Valid length
        state.block_index = 0;

        let events = close_current_block_with_signature(&mut state);

        // Should have signature_delta and content_block_stop
        assert!(events.len() >= 2);
        assert!(events.iter().any(|e| {
            if let StreamEvent::ContentBlockDelta { delta, .. } = e {
                delta.is_signature()
            } else {
                false
            }
        }));
        assert!(events.iter().any(|e| e.is_content_block_stop()));

        // State should be updated
        assert_eq!(state.block_index, 1);
        assert!(state.current_block_type.is_none());
        assert!(state.current_thinking_signature.is_empty());
    }

    #[test]
    fn test_emit_message_start() {
        let state = StreamState::new("claude-sonnet-4-5".to_string());

        let event = emit_message_start(&state);

        assert!(event.is_message_start());
        if let StreamEvent::MessageStart { message } = event {
            assert!(message.id.starts_with("msg_"));
            assert_eq!(message.model, "claude-sonnet-4-5");
        } else {
            panic!("Expected MessageStart event");
        }
    }

    #[test]
    fn test_sse_data_deserialization() {
        let json = r#"{"candidates":[{"content":{"parts":[{"text":"Hello"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":5}}"#;
        let data: SseData = serde_json::from_str(json).unwrap();

        assert!(data.candidates.is_some());
        assert!(data.usage_metadata.is_some());
        assert_eq!(data.usage_metadata.as_ref().unwrap().prompt_token_count, 10);
    }

    #[test]
    fn test_sse_data_nested_response() {
        let json = r#"{"response":{"candidates":[{"content":{"parts":[{"text":"Hi"}]}}]}}"#;
        let data: SseData = serde_json::from_str(json).unwrap();

        assert!(data.response.is_some());
        let inner = data.response.as_ref().unwrap();
        assert!(inner.candidates.is_some());
    }
}
