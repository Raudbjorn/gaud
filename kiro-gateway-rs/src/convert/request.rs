//! Convert Anthropic MessagesRequest to Kiro API payload.

use uuid::Uuid;

use crate::config::{API_ORIGIN, MAX_TOOL_DESCRIPTION_LENGTH, MAX_TOOL_NAME_LENGTH};
use crate::convert::content;
use crate::convert::schema::sanitize_json_schema;
use crate::error::{Error, Result};
use crate::models::kiro::{InputSchema, KiroToolSpec, ToolSpecification};
use crate::models::request::{MessagesRequest, Message, Role, SystemPrompt, Tool};

/// Convert a `MessagesRequest` into a Kiro API JSON payload.
pub fn build_kiro_payload(
    request: &MessagesRequest,
    model_id: &str,
    profile_arn: Option<&str>,
) -> Result<serde_json::Value> {
    if request.messages.is_empty() {
        return Err(Error::EmptyMessages);
    }

    let conversation_id = Uuid::new_v4().to_string();

    // Process messages: merge adjacent same-role, ensure alternating
    let processed = process_messages(&request.messages);
    if processed.is_empty() {
        return Err(Error::EmptyMessages);
    }

    // The last user message becomes currentMessage; everything before is history
    let (history_msgs, current_msg) = processed.split_at(processed.len() - 1);
    let current = &current_msg[0];

    // Build current message
    let current_text = content::extract_text(&current.content);
    let current_images = content::extract_images(&current.content);
    let current_tool_results = content::extract_tool_results(&current.content);

    let mut current_message = serde_json::json!({
        "userInputMessage": {
            "content": current_text,
            "modelId": model_id,
            "origin": API_ORIGIN,
        }
    });

    if !current_images.is_empty() {
        current_message["userInputMessage"]["images"] =
            serde_json::to_value(&current_images).unwrap_or_default();
    }

    // Build tool context
    let mut system_overflow = String::new();
    let mut context = serde_json::Map::new();

    if let Some(tools) = &request.tools {
        let (kiro_tools, overflow) = convert_tools(tools);
        if !kiro_tools.is_empty() {
            context.insert(
                "tools".to_string(),
                serde_json::to_value(&kiro_tools).unwrap_or_default(),
            );
        }
        if !overflow.is_empty() {
            system_overflow = overflow;
        }
    }

    if !current_tool_results.is_empty() {
        context.insert(
            "toolResults".to_string(),
            serde_json::to_value(&current_tool_results).unwrap_or_default(),
        );
    }

    if !context.is_empty() {
        current_message["userInputMessage"]["userInputMessageContext"] =
            serde_json::Value::Object(context);
    }

    // Build system prompt (combine explicit system + tool description overflow)
    let system_text = build_system_prompt(request.system.as_ref(), &system_overflow);

    // Inject thinking tags into content if thinking is enabled
    let final_content = if request.thinking.is_some() {
        inject_thinking_tags(&current_text, &system_text)
    } else if !system_text.is_empty() {
        // Prepend system prompt to the user content
        format!("{}\n\n{}", system_text, current_text)
    } else {
        current_text.clone()
    };

    if final_content != current_text {
        current_message["userInputMessage"]["content"] =
            serde_json::Value::String(final_content);
    }

    // Build history
    let history: Vec<serde_json::Value> = history_msgs
        .iter()
        .map(|msg| content::message_to_history_entry(msg, model_id))
        .collect();

    // Assemble final payload
    let mut payload = serde_json::json!({
        "conversationState": {
            "chatTriggerType": "MANUAL",
            "conversationId": conversation_id,
            "currentMessage": current_message,
        }
    });

    if !history.is_empty() {
        payload["conversationState"]["history"] = serde_json::Value::Array(history);
    }

    if let Some(arn) = profile_arn {
        payload["profileArn"] = serde_json::Value::String(arn.to_string());
    }

    Ok(payload)
}

/// Process messages: merge adjacent same-role, ensure alternating user/assistant.
fn process_messages(messages: &[Message]) -> Vec<Message> {
    if messages.is_empty() {
        return Vec::new();
    }

    let mut result: Vec<Message> = Vec::new();

    for msg in messages {
        let role = match msg.role {
            Role::System => Role::User, // Normalize system to user
            other => other,
        };

        // Merge with previous if same role (preserve all content blocks, not just text)
        if let Some(last) = result.last_mut() {
            if last.role == role {
                last.content = merge_content(&last.content, &msg.content);
                continue;
            }
        }

        result.push(Message {
            role,
            content: msg.content.clone(),
        });
    }

    // Ensure the conversation starts with user and alternates
    ensure_alternating(&mut result);

    // Ensure last message is from user (required by Kiro)
    if result.last().is_some_and(|m| m.role != Role::User) {
        result.push(Message {
            role: Role::User,
            content: crate::models::request::MessageContent::Text("Continue.".to_string()),
        });
    }

    result
}

/// Merge two message contents, preserving all content blocks (text, images, tool results, etc.).
fn merge_content(
    existing: &crate::models::request::MessageContent,
    new: &crate::models::request::MessageContent,
) -> crate::models::request::MessageContent {
    use crate::models::request::{ContentBlock, MessageContent};

    let mut blocks: Vec<ContentBlock> = match existing {
        MessageContent::Text(t) => vec![ContentBlock::Text { text: t.clone() }],
        MessageContent::Blocks(b) => b.clone(),
    };

    match new {
        MessageContent::Text(t) => blocks.push(ContentBlock::Text { text: t.clone() }),
        MessageContent::Blocks(b) => blocks.extend(b.iter().cloned()),
    }

    MessageContent::Blocks(blocks)
}

/// Ensure messages alternate between user and assistant by inserting fillers.
fn ensure_alternating(messages: &mut Vec<Message>) {
    let mut i = 1;
    while i < messages.len() {
        if messages[i].role == messages[i - 1].role {
            let filler_role = if messages[i].role == Role::User {
                Role::Assistant
            } else {
                Role::User
            };
            let filler_text = if filler_role == Role::Assistant {
                "Understood."
            } else {
                "Continue."
            };
            messages.insert(
                i,
                Message {
                    role: filler_role,
                    content: crate::models::request::MessageContent::Text(filler_text.to_string()),
                },
            );
            i += 2;
        } else {
            i += 1;
        }
    }
}

/// Convert Anthropic tools to Kiro format, handling overflow for long descriptions.
fn convert_tools(tools: &[Tool]) -> (Vec<KiroToolSpec>, String) {
    let mut kiro_tools = Vec::new();
    let mut overflow_parts = Vec::new();

    for tool in tools {
        let description = tool.description.clone().unwrap_or_default();

        // Truncate tool name if needed
        let name = if tool.name.len() > MAX_TOOL_NAME_LENGTH {
            tool.name[..MAX_TOOL_NAME_LENGTH].to_string()
        } else {
            tool.name.clone()
        };

        // If description is too long, move it to system prompt overflow
        let (tool_description, overflow) = if description.len() > MAX_TOOL_DESCRIPTION_LENGTH {
            let short = format!(
                "{}... (full description in system prompt)",
                &description[..200]
            );
            let full = format!(
                "Tool '{}' full description:\n{}",
                name, description
            );
            (short, Some(full))
        } else {
            (description, None)
        };

        if let Some(overflow_text) = overflow {
            overflow_parts.push(overflow_text);
        }

        let schema = sanitize_json_schema(&tool.input_schema);

        kiro_tools.push(KiroToolSpec {
            tool_specification: ToolSpecification {
                name,
                description: tool_description,
                input_schema: InputSchema { json: schema },
            },
        });
    }

    let overflow = if overflow_parts.is_empty() {
        String::new()
    } else {
        overflow_parts.join("\n\n")
    };

    (kiro_tools, overflow)
}

/// Build the system prompt from explicit system + overflow.
fn build_system_prompt(system: Option<&SystemPrompt>, overflow: &str) -> String {
    let mut parts = Vec::new();

    if let Some(sys) = system {
        let text = sys.text();
        if !text.is_empty() {
            parts.push(text);
        }
    }

    if !overflow.is_empty() {
        parts.push(overflow.to_string());
    }

    parts.join("\n\n")
}

/// Inject thinking tags into content when extended thinking is enabled.
fn inject_thinking_tags(content: &str, system: &str) -> String {
    let mut parts = Vec::new();

    if !system.is_empty() {
        parts.push(system.to_string());
    }

    parts.push(
        "<instruction>Use `<antThinking>` tags to show your reasoning before responding.</instruction>"
            .to_string(),
    );
    parts.push(content.to_string());

    parts.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::request::{MessageContent, Message, Role};

    #[test]
    fn test_process_messages_merge_adjacent() {
        let messages = vec![
            Message {
                role: Role::User,
                content: MessageContent::Text("Hello".into()),
            },
            Message {
                role: Role::User,
                content: MessageContent::Text("World".into()),
            },
        ];
        let result = process_messages(&messages);
        assert_eq!(result.len(), 1);
        // Merged as blocks: text from both messages preserved
        assert_eq!(result[0].content.text(), "HelloWorld");
    }

    #[test]
    fn test_process_messages_merge_preserves_non_text_blocks() {
        use crate::models::request::{ContentBlock, ImageSource};

        let messages = vec![
            Message {
                role: Role::User,
                content: MessageContent::Blocks(vec![
                    ContentBlock::Text { text: "Look at this:".into() },
                    ContentBlock::Image {
                        source: ImageSource {
                            source_type: "base64".into(),
                            media_type: "image/png".into(),
                            data: "iVBOR".into(),
                        },
                    },
                ]),
            },
            Message {
                role: Role::User,
                content: MessageContent::Text("What do you see?".into()),
            },
        ];
        let result = process_messages(&messages);
        assert_eq!(result.len(), 1);
        match &result[0].content {
            MessageContent::Blocks(blocks) => {
                assert_eq!(blocks.len(), 3); // text + image + text
                assert!(matches!(&blocks[1], ContentBlock::Image { .. }));
            }
            _ => panic!("Expected Blocks content after merge"),
        }
    }

    #[test]
    fn test_process_messages_alternating() {
        let messages = vec![
            Message {
                role: Role::User,
                content: MessageContent::Text("Hi".into()),
            },
            Message {
                role: Role::Assistant,
                content: MessageContent::Text("Hey".into()),
            },
            Message {
                role: Role::User,
                content: MessageContent::Text("What?".into()),
            },
        ];
        let result = process_messages(&messages);
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn test_process_messages_ensures_user_last() {
        let messages = vec![
            Message {
                role: Role::User,
                content: MessageContent::Text("Hi".into()),
            },
            Message {
                role: Role::Assistant,
                content: MessageContent::Text("Hey".into()),
            },
        ];
        let result = process_messages(&messages);
        assert_eq!(result.last().unwrap().role, Role::User);
    }

    #[test]
    fn test_build_kiro_payload_minimal() {
        let request = MessagesRequest {
            model: "claude-sonnet-4.5".into(),
            max_tokens: 1024,
            messages: vec![Message {
                role: Role::User,
                content: MessageContent::Text("Hello".into()),
            }],
            system: None,
            tools: None,
            tool_choice: None,
            stream: false,
            temperature: None,
            top_p: None,
            stop_sequences: None,
            thinking: None,
        };

        let payload = build_kiro_payload(&request, "claude-sonnet-4.5", None).unwrap();
        assert!(payload.get("conversationState").is_some());
        let current = &payload["conversationState"]["currentMessage"]["userInputMessage"];
        assert_eq!(current["content"].as_str().unwrap(), "Hello");
        assert_eq!(current["modelId"].as_str().unwrap(), "claude-sonnet-4.5");
    }
}
