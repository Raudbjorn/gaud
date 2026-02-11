//! Convert Kiro stream events to Anthropic Messages API responses.

use uuid::Uuid;

use crate::models::kiro::KiroStreamEvent;
use crate::models::response::{MessagesResponse, ResponseContentBlock, StopReason, Usage};
use crate::models::stream::{ContentDelta, MessageDelta, PartialMessage, StreamEvent};

/// Tracks thinking tag parsing state across chunk boundaries.
#[derive(Debug, Default)]
struct ThinkingState {
    /// Whether we are currently inside a thinking block.
    inside_thinking: bool,
    /// Buffer for partial tag matches at chunk boundaries.
    tag_buffer: String,
}

impl ThinkingState {
    /// Process a text chunk that may contain thinking tags split across boundaries.
    ///
    /// Returns `(thinking_text, regular_text)` — either or both may be empty.
    fn process(&mut self, text: &str) -> (String, String) {
        let mut thinking_out = String::new();
        let mut regular_out = String::new();

        // Prepend any leftover partial tag from a previous chunk
        let input = if self.tag_buffer.is_empty() {
            text.to_string()
        } else {
            let combined = format!("{}{}", self.tag_buffer, text);
            self.tag_buffer.clear();
            combined
        };

        let mut remaining = input.as_str();

        while !remaining.is_empty() {
            if self.inside_thinking {
                // Look for closing tag
                if let Some(pos) = remaining.find("</antThinking>") {
                    thinking_out.push_str(&remaining[..pos]);
                    remaining = &remaining[pos + "</antThinking>".len()..];
                    self.inside_thinking = false;
                } else {
                    // No complete closing tag found. Check if the end of the
                    // chunk could be the start of a partial closing tag.
                    let tag = "</antThinking>";
                    let mut split_at = remaining.len();
                    for i in 1..tag.len().min(remaining.len()) {
                        let tail = &remaining[remaining.len() - i..];
                        if tag.starts_with(tail) && tail.starts_with('<') {
                            split_at = remaining.len() - i;
                            break;
                        }
                    }
                    if split_at < remaining.len() {
                        thinking_out.push_str(&remaining[..split_at]);
                        self.tag_buffer = remaining[split_at..].to_string();
                    } else {
                        thinking_out.push_str(remaining);
                    }
                    remaining = "";
                }
            } else {
                // Look for opening tag
                if let Some(pos) = remaining.find("<antThinking>") {
                    regular_out.push_str(&remaining[..pos]);
                    remaining = &remaining[pos + "<antThinking>".len()..];
                    self.inside_thinking = true;
                } else if remaining.len() < "<antThinking>".len() {
                    // Check if the end of the chunk could be the start of an opening tag
                    let check_len = "<antThinking>".len() - 1;
                    let tail_start = remaining.len().saturating_sub(check_len);
                    let tail = &remaining[tail_start..];
                    if "<antThinking>".starts_with(tail) && tail.starts_with('<') {
                        regular_out.push_str(&remaining[..tail_start]);
                        self.tag_buffer = tail.to_string();
                        remaining = "";
                    } else {
                        regular_out.push_str(remaining);
                        remaining = "";
                    }
                } else {
                    // Check if chunk ends with a partial opening tag
                    let check_len = "<antThinking>".len() - 1;
                    let tail_start = remaining.len().saturating_sub(check_len);
                    let tail = &remaining[tail_start..];
                    let mut found_partial = false;
                    for i in 0..tail.len() {
                        let candidate = &tail[i..];
                        if candidate.starts_with('<') && "<antThinking>".starts_with(candidate) {
                            regular_out.push_str(&remaining[..tail_start + i]);
                            self.tag_buffer = candidate.to_string();
                            found_partial = true;
                            break;
                        }
                    }
                    if !found_partial {
                        regular_out.push_str(remaining);
                    }
                    remaining = "";
                }
            }
        }

        (thinking_out, regular_out)
    }
}

/// Accumulates Kiro stream events into a complete Messages response.
pub struct ResponseAccumulator {
    /// Response ID.
    id: String,
    /// Model name.
    model: String,
    /// Accumulated text content.
    text: String,
    /// Accumulated thinking content.
    thinking: Option<String>,
    /// Accumulated tool uses.
    tool_uses: Vec<ToolUseAccumulator>,
    /// Current tool being accumulated.
    current_tool: Option<ToolUseAccumulator>,
    /// Usage data.
    input_tokens: u32,
    output_tokens: u32,
    /// Context usage percentage.
    context_usage_pct: Option<f64>,
    /// Whether we have received real usage data from the server.
    has_real_usage: bool,
    /// Whether the stream has ended.
    finished: bool,
    /// Current content block index.
    block_index: usize,
    /// Thinking tag parser state (survives across chunks).
    thinking_state: ThinkingState,
}

struct ToolUseAccumulator {
    id: String,
    name: String,
    input_json: String,
}

impl ResponseAccumulator {
    /// Create a new accumulator for the given model.
    pub fn new(model: &str) -> Self {
        Self {
            id: format!("msg_{}", Uuid::new_v4().simple()),
            model: model.to_string(),
            text: String::new(),
            thinking: None,
            tool_uses: Vec::new(),
            current_tool: None,
            input_tokens: 0,
            output_tokens: 0,
            context_usage_pct: None,
            has_real_usage: false,
            finished: false,
            block_index: 0,
            thinking_state: ThinkingState::default(),
        }
    }

    /// Process a Kiro stream event and return any Anthropic stream events to emit.
    pub fn process_event(&mut self, event: KiroStreamEvent) -> Vec<StreamEvent> {
        match event {
            KiroStreamEvent::Content(text) => {
                let (thinking_text, regular_text) = self.thinking_state.process(&text);
                let mut events = Vec::new();

                if !thinking_text.is_empty() {
                    let thinking = self.thinking.get_or_insert_with(String::new);
                    thinking.push_str(&thinking_text);
                    events.push(StreamEvent::ContentBlockDelta {
                        index: self.block_index,
                        delta: ContentDelta::ThinkingDelta {
                            thinking: thinking_text,
                        },
                    });
                }

                if !regular_text.is_empty() {
                    self.text.push_str(&regular_text);
                    if !self.has_real_usage {
                        self.output_tokens += estimate_tokens(&regular_text);
                    }
                    events.push(StreamEvent::ContentBlockDelta {
                        index: self.block_index,
                        delta: ContentDelta::TextDelta { text: regular_text },
                    });
                }

                events
            }
            KiroStreamEvent::ToolStart {
                name,
                tool_use_id,
                input,
            } => {
                // Finalize any current tool
                self.finalize_current_tool();

                let tool_id = if tool_use_id.is_empty() {
                    format!("toolu_{}", Uuid::new_v4().simple())
                } else {
                    tool_use_id
                };

                self.block_index += 1;

                let has_initial_input = !input.is_empty();
                self.current_tool = Some(ToolUseAccumulator {
                    id: tool_id.clone(),
                    name: name.clone(),
                    input_json: input.clone(),
                });

                let mut events = vec![StreamEvent::ContentBlockStart {
                    index: self.block_index,
                    content_block: ResponseContentBlock::ToolUse {
                        id: tool_id,
                        name,
                        input: serde_json::Value::Object(serde_json::Map::new()),
                    },
                }];

                // Emit InputJsonDelta for any pre-populated arguments so that
                // clients streaming tool calls receive the initial input.
                if has_initial_input {
                    events.push(StreamEvent::ContentBlockDelta {
                        index: self.block_index,
                        delta: ContentDelta::InputJsonDelta {
                            partial_json: input,
                        },
                    });
                }

                events
            }
            KiroStreamEvent::ToolInput(input) => {
                if let Some(tool) = &mut self.current_tool {
                    tool.input_json.push_str(&input);
                }
                vec![StreamEvent::ContentBlockDelta {
                    index: self.block_index,
                    delta: ContentDelta::InputJsonDelta {
                        partial_json: input,
                    },
                }]
            }
            KiroStreamEvent::Stop { reason } => {
                // Only finalize tool if we were actually building one
                if self.current_tool.is_some() {
                    self.finalize_current_tool();
                }
                let events = vec![StreamEvent::ContentBlockStop {
                    index: self.block_index,
                }];
                // Only advance block index if this was a tool-specific stop
                if reason != "end_turn" {
                    self.block_index += 1;
                }
                events
            }
            KiroStreamEvent::Usage(usage_data) => {
                if let Some(input) = usage_data.get("inputTokenCount").and_then(|v| v.as_u64()) {
                    self.input_tokens = input as u32;
                }
                if let Some(output) = usage_data.get("outputTokenCount").and_then(|v| v.as_u64()) {
                    self.output_tokens = output as u32;
                }
                self.has_real_usage = true;
                Vec::new()
            }
            KiroStreamEvent::ContextUsage(pct) => {
                self.context_usage_pct = Some(pct);
                Vec::new()
            }
        }
    }

    /// Build the initial message_start event.
    pub fn message_start_event(&self) -> StreamEvent {
        StreamEvent::MessageStart {
            message: PartialMessage {
                id: self.id.clone(),
                message_type: "message".to_string(),
                role: "assistant".to_string(),
                model: self.model.clone(),
                usage: Usage {
                    input_tokens: self.input_tokens,
                    output_tokens: 0,
                    cache_creation_input_tokens: None,
                    cache_read_input_tokens: None,
                },
            },
        }
    }

    /// Build the content_block_start event for the initial text block.
    pub fn text_block_start_event(&self) -> StreamEvent {
        StreamEvent::ContentBlockStart {
            index: 0,
            content_block: ResponseContentBlock::Text {
                text: String::new(),
            },
        }
    }

    /// Build the final message_delta + message_stop events.
    pub fn finish_events(&mut self) -> Vec<StreamEvent> {
        self.finalize_current_tool();
        self.finished = true;

        let stop_reason = if self.tool_uses.is_empty() {
            StopReason::EndTurn
        } else {
            StopReason::ToolUse
        };

        vec![
            StreamEvent::ContentBlockStop {
                index: self.block_index,
            },
            StreamEvent::MessageDelta {
                delta: MessageDelta {
                    stop_reason: Some(stop_reason),
                    stop_sequence: None,
                },
                usage: Some(Usage {
                    input_tokens: self.input_tokens,
                    output_tokens: self.output_tokens,
                    cache_creation_input_tokens: None,
                    cache_read_input_tokens: None,
                }),
            },
            StreamEvent::MessageStop,
        ]
    }

    /// Build a complete `MessagesResponse` from accumulated data.
    pub fn into_response(mut self) -> MessagesResponse {
        self.finalize_current_tool();

        let mut content = Vec::new();

        // Add thinking block if present
        if let Some(thinking) = &self.thinking {
            if !thinking.is_empty() {
                content.push(ResponseContentBlock::Thinking {
                    thinking: thinking.clone(),
                });
            }
        }

        // Add text block
        if !self.text.is_empty() {
            content.push(ResponseContentBlock::Text {
                text: self.text.clone(),
            });
        }

        // Add tool use blocks
        for tool in &self.tool_uses {
            let input: serde_json::Value =
                serde_json::from_str(&tool.input_json).unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
            content.push(ResponseContentBlock::ToolUse {
                id: tool.id.clone(),
                name: tool.name.clone(),
                input,
            });
        }

        let stop_reason = if self.tool_uses.is_empty() {
            StopReason::EndTurn
        } else {
            StopReason::ToolUse
        };

        MessagesResponse {
            id: self.id,
            response_type: "message".to_string(),
            role: "assistant".to_string(),
            content,
            model: self.model,
            stop_reason: Some(stop_reason),
            stop_sequence: None,
            usage: Usage {
                input_tokens: self.input_tokens,
                output_tokens: self.output_tokens,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            },
        }
    }

    fn finalize_current_tool(&mut self) {
        if let Some(tool) = self.current_tool.take() {
            self.tool_uses.push(tool);
        }
    }
}

/// Rough token estimation (4 chars per token).
/// Only used as a fallback when real usage data has not been received from the server.
fn estimate_tokens(text: &str) -> u32 {
    (text.len() / 4).max(1) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_thinking_state_simple() {
        let mut state = ThinkingState::default();
        let (thinking, regular) = state.process("Hello world");
        assert_eq!(thinking, "");
        assert_eq!(regular, "Hello world");
    }

    #[test]
    fn test_thinking_state_complete_tags() {
        let mut state = ThinkingState::default();
        let (thinking, regular) = state.process("<antThinking>deep thought</antThinking>result");
        assert_eq!(thinking, "deep thought");
        assert_eq!(regular, "result");
    }

    #[test]
    fn test_thinking_state_split_across_chunks() {
        let mut state = ThinkingState::default();

        // Opening tag split across chunks
        let (t1, r1) = state.process("before<antThi");
        assert_eq!(r1, "before");
        assert_eq!(t1, "");

        let (t2, r2) = state.process("nking>inside thought");
        assert_eq!(t2, "inside thought");
        assert_eq!(r2, "");

        // Closing tag split across chunks
        let (t3, r3) = state.process(" more</antTh");
        assert_eq!(t3, " more");
        assert_eq!(r3, "");

        let (t4, r4) = state.process("inking>after");
        assert_eq!(t4, "");
        assert_eq!(r4, "after");
    }

    #[test]
    fn test_thinking_state_no_false_positives() {
        let mut state = ThinkingState::default();
        let (thinking, regular) = state.process("This is <not> a thinking tag");
        assert_eq!(thinking, "");
        assert_eq!(regular, "This is <not> a thinking tag");
    }

    #[test]
    fn test_thinking_state_opening_tag_almost_complete() {
        let mut state = ThinkingState::default();
        // Chunk ends with almost-complete opening tag
        let (t1, r1) = state.process("hello<antThinkin");
        assert_eq!(t1, "");
        assert_eq!(r1, "hello");

        let (t2, r2) = state.process("g>thinking content");
        assert_eq!(t2, "thinking content");
        assert_eq!(r2, "");
    }

    #[test]
    fn test_thinking_state_closing_tag_split_at_angle_bracket() {
        let mut state = ThinkingState::default();
        state.process("<antThinking>start");

        // Closing tag split exactly at the >
        let (t1, _r1) = state.process(" more</antThinking");
        assert_eq!(t1, " more");

        let (t2, r2) = state.process(">after");
        assert_eq!(t2, "");
        assert_eq!(r2, "after");
    }

    #[test]
    fn test_thinking_state_empty_chunks() {
        let mut state = ThinkingState::default();
        let (t, r) = state.process("");
        assert_eq!(t, "");
        assert_eq!(r, "");
    }

    #[test]
    fn test_thinking_state_single_angle_bracket() {
        let mut state = ThinkingState::default();
        let (t, r) = state.process("text with < in it");
        assert_eq!(t, "");
        assert_eq!(r, "text with < in it");
    }

    #[test]
    fn test_thinking_state_multiple_thinking_blocks() {
        let mut state = ThinkingState::default();
        let (t, r) = state.process("<antThinking>first</antThinking>middle<antThinking>second</antThinking>end");
        assert_eq!(t, "firstsecond");
        assert_eq!(r, "middleend");
    }

    #[test]
    fn test_tool_start_with_initial_input() {
        let mut acc = ResponseAccumulator::new("test-model");
        let events = acc.process_event(KiroStreamEvent::ToolStart {
            name: "search".to_string(),
            tool_use_id: "tool_1".to_string(),
            input: r#"{"q":"test"}"#.to_string(),
        });
        // Should emit both ContentBlockStart AND InputJsonDelta
        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], StreamEvent::ContentBlockStart { .. }));
        assert!(matches!(
            events[1],
            StreamEvent::ContentBlockDelta {
                delta: ContentDelta::InputJsonDelta { .. },
                ..
            }
        ));
    }

    #[test]
    fn test_tool_start_without_initial_input() {
        let mut acc = ResponseAccumulator::new("test-model");
        let events = acc.process_event(KiroStreamEvent::ToolStart {
            name: "search".to_string(),
            tool_use_id: "tool_1".to_string(),
            input: String::new(),
        });
        // Should emit only ContentBlockStart
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], StreamEvent::ContentBlockStart { .. }));
    }
}
