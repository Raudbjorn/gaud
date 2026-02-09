//! Content block conversion between Anthropic and Kiro formats.

use crate::models::kiro::{
    KiroImage, KiroImageSource, KiroTextContent, KiroToolResult, KiroToolUse,
};
use crate::models::request::{ContentBlock, ImageSource, Message, MessageContent, Role};

/// Extract plain text from a message's content.
pub fn extract_text(content: &MessageContent) -> String {
    content.text()
}

/// Extract images from a message's content blocks.
pub fn extract_images(content: &MessageContent) -> Vec<KiroImage> {
    match content {
        MessageContent::Blocks(blocks) => blocks
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Image { source } => Some(image_to_kiro(source)),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Convert an Anthropic image source to Kiro format.
fn image_to_kiro(source: &ImageSource) -> KiroImage {
    // Map media type to Kiro format string
    let format = match source.media_type.as_str() {
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        _ => "jpeg",
    };

    KiroImage {
        format: format.to_string(),
        source: KiroImageSource {
            bytes: source.data.clone(),
        },
    }
}

/// Extract tool use blocks from a message.
pub fn extract_tool_uses(content: &MessageContent) -> Vec<KiroToolUse> {
    match content {
        MessageContent::Blocks(blocks) => blocks
            .iter()
            .filter_map(|b| match b {
                ContentBlock::ToolUse { id, name, input } => Some(KiroToolUse {
                    name: name.clone(),
                    input: input.clone(),
                    tool_use_id: id.clone(),
                }),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Extract tool result blocks from a message.
pub fn extract_tool_results(content: &MessageContent) -> Vec<KiroToolResult> {
    match content {
        MessageContent::Blocks(blocks) => blocks
            .iter()
            .filter_map(|b| match b {
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => {
                    let text = content.text();
                    let status = if *is_error { "error" } else { "success" };
                    Some(KiroToolResult {
                        content: vec![KiroTextContent { text }],
                        status: status.to_string(),
                        tool_use_id: tool_use_id.clone(),
                    })
                }
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Extract thinking text from message content.
pub fn extract_thinking(content: &MessageContent) -> Option<String> {
    match content {
        MessageContent::Blocks(blocks) => {
            let thinking: Vec<&str> = blocks
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Thinking { thinking } => Some(thinking.as_str()),
                    _ => None,
                })
                .collect();
            if thinking.is_empty() {
                None
            } else {
                Some(thinking.join("\n"))
            }
        }
        _ => None,
    }
}

/// Convert a message to a Kiro history entry.
pub fn message_to_history_entry(msg: &Message, model_id: &str) -> serde_json::Value {
    match msg.role {
        Role::User => {
            let text = extract_text(&msg.content);
            let images = extract_images(&msg.content);
            let tool_results = extract_tool_results(&msg.content);

            let mut entry = serde_json::json!({
                "userInputMessage": {
                    "content": text,
                    "modelId": model_id,
                    "origin": crate::config::API_ORIGIN,
                }
            });

            if !images.is_empty() {
                entry["userInputMessage"]["images"] =
                    serde_json::to_value(&images).unwrap_or_default();
            }

            if !tool_results.is_empty() {
                entry["userInputMessage"]["userInputMessageContext"] = serde_json::json!({
                    "toolResults": serde_json::to_value(&tool_results).unwrap_or_default()
                });
            }

            entry
        }
        Role::Assistant => {
            let text = extract_text(&msg.content);
            let tool_uses = extract_tool_uses(&msg.content);
            let thinking = extract_thinking(&msg.content);

            // Wrap thinking in XML tags if present
            let content = if let Some(thinking_text) = thinking {
                format!(
                    "<antThinking>\n{}\n</antThinking>\n{}",
                    thinking_text, text
                )
            } else {
                text
            };

            let mut entry = serde_json::json!({
                "assistantResponseMessage": {
                    "content": content,
                }
            });

            if !tool_uses.is_empty() {
                entry["assistantResponseMessage"]["toolUses"] =
                    serde_json::to_value(&tool_uses).unwrap_or_default();
            }

            entry
        }
        Role::System => {
            // System messages get folded into the user message as a prefix
            let text = extract_text(&msg.content);
            serde_json::json!({
                "userInputMessage": {
                    "content": text,
                    "modelId": model_id,
                    "origin": crate::config::API_ORIGIN,
                }
            })
        }
    }
}
