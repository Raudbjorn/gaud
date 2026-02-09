//! Convert Kiro stream events to Anthropic Messages API responses.

use uuid::Uuid;

use crate::models::kiro::KiroStreamEvent;
use crate::models::response::{MessagesResponse, ResponseContentBlock, StopReason, Usage};
use crate::models::stream::{ContentDelta, MessageDelta, PartialMessage, StreamEvent};

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
    /// Whether the stream has ended.
    finished: bool,
    /// Current content block index.
    block_index: usize,
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
            finished: false,
            block_index: 0,
        }
    }

    /// Process a Kiro stream event and return any Anthropic stream events to emit.
    pub fn process_event(&mut self, event: KiroStreamEvent) -> Vec<StreamEvent> {
        match event {
            KiroStreamEvent::Content(text) => {
                // Check for thinking tags
                if text.contains("<antThinking>") || text.contains("</antThinking>") {
                    self.process_thinking_content(&text)
                } else {
                    self.text.push_str(&text);
                    self.output_tokens += estimate_tokens(&text);
                    vec![StreamEvent::ContentBlockDelta {
                        index: self.block_index,
                        delta: ContentDelta::TextDelta { text },
                    }]
                }
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
                self.current_tool = Some(ToolUseAccumulator {
                    id: tool_id.clone(),
                    name: name.clone(),
                    input_json: input,
                });

                vec![StreamEvent::ContentBlockStart {
                    index: self.block_index,
                    content_block: ResponseContentBlock::ToolUse {
                        id: tool_id,
                        name,
                        input: serde_json::Value::Object(serde_json::Map::new()),
                    },
                }]
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
            KiroStreamEvent::ToolStop => {
                self.finalize_current_tool();
                let events = vec![StreamEvent::ContentBlockStop {
                    index: self.block_index,
                }];
                self.block_index += 1;
                events
            }
            KiroStreamEvent::Usage(usage_data) => {
                if let Some(input) = usage_data.get("inputTokenCount").and_then(|v| v.as_u64()) {
                    self.input_tokens = input as u32;
                }
                if let Some(output) = usage_data.get("outputTokenCount").and_then(|v| v.as_u64()) {
                    self.output_tokens = output as u32;
                }
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

    fn process_thinking_content(&mut self, text: &str) -> Vec<StreamEvent> {
        // Simple tag-aware splitting
        let thinking = self.thinking.get_or_insert_with(String::new);
        let clean = text
            .replace("<antThinking>", "")
            .replace("</antThinking>", "");
        thinking.push_str(&clean);

        vec![StreamEvent::ContentBlockDelta {
            index: self.block_index,
            delta: ContentDelta::ThinkingDelta {
                thinking: clean,
            },
        }]
    }

    fn finalize_current_tool(&mut self) {
        if let Some(tool) = self.current_tool.take() {
            self.tool_uses.push(tool);
        }
    }
}

/// Rough token estimation (4 chars per token).
fn estimate_tokens(text: &str) -> u32 {
    (text.len() / 4).max(1) as u32
}
