//! Request conversion from Anthropic to Google format.
//!
//! This module converts `MessagesRequest` (Anthropic format) to `GoogleRequest`
//! (Google Generative AI format) for use with the Cloud Code API.
//!
//! ## Conversion Details
//!
//! ### Messages
//! - Messages are converted to `contents` array
//! - Role mapping: `user` -> `user`, `assistant` -> `model`
//! - Content blocks are converted to Parts (see `content` module)
//!
//! ### System Prompt
//! - Converted to `systemInstruction` with `user` role (as per Google API requirement)
//! - Supports both string and block formats
//!
//! ### Generation Config
//! - `max_tokens` -> `maxOutputTokens`
//! - `temperature`, `top_p`, `top_k`, `stop_sequences` passed through
//!
//! ### Tools
//! - Converted to `functionDeclarations`
//! - Schemas are sanitized for Google API compatibility
//!
//! ### Thinking
//! - For Claude thinking models: `{ budgetTokens: N }`
//! - For Gemini thinking models: `{ includeThoughts: true, thinkingBudget: N }`

use crate::gemini::constants::{
    get_model_family, is_thinking_model, ModelFamily, GEMINI_MAX_OUTPUT_TOKENS,
};
use crate::gemini::models::google::{
    Content, FunctionDeclaration, GenerationConfig, GoogleRequest, GoogleThinkingConfig,
    GoogleTool, Part, ToolConfig,
};
use crate::gemini::models::request::{Message, MessageContent, MessagesRequest, SystemPrompt};

use super::content::{convert_content_to_parts, convert_role, text_to_parts};
use super::schema::sanitize_schema;

/// Convert an Anthropic `MessagesRequest` to a Google `GoogleRequest`.
///
/// This is the main entry point for request conversion. It handles all
/// aspects of the conversion including messages, system prompt, tools,
/// and thinking configuration.
///
/// # Arguments
///
/// * `request` - The Anthropic Messages API request
///
/// # Returns
///
/// A `GoogleRequest` suitable for the Cloud Code API.
///
/// # Example
///
/// ```rust,ignore
/// use gaud::gemini::convert::convert_request;
/// use gaud::gemini::MessagesRequest;
///
/// let anthropic_request = MessagesRequest::simple("claude-sonnet-4-5", 1024, "Hello!");
/// let google_request = convert_request(&anthropic_request);
/// ```
pub(crate) fn convert_request(request: &MessagesRequest) -> GoogleRequest {
    let model_family = get_model_family(&request.model);
    let is_claude = model_family == ModelFamily::Claude;
    let is_gemini = model_family == ModelFamily::Gemini;
    let is_thinking = is_thinking_model(&request.model);

    let mut google_request = GoogleRequest::new();

    // Convert system instruction
    if let Some(system) = &request.system {
        let system_parts = convert_system_prompt(system);
        if !system_parts.is_empty() {
            // For Claude thinking models with tools, add interleaved thinking hint
            let mut parts = system_parts;
            if is_claude && is_thinking && request.has_tools() {
                let hint = "Interleaved thinking is enabled. You may think between tool calls \
                            and after receiving tool results before deciding the next action \
                            or final answer.";
                if let Some(last) = parts.last_mut() {
                    if let Some(text) = &last.text {
                        last.text = Some(format!("{}\n\n{}", text, hint));
                    }
                } else {
                    parts.push(Part::text(hint));
                }
            }

            google_request.system_instruction = Some(Content::system(parts));
        }
    }

    // Convert messages to contents
    google_request.contents = convert_messages(&request.messages, &request.model);

    // Build generation config
    let mut gen_config = GenerationConfig::new(request.max_tokens);

    if let Some(temp) = request.temperature {
        gen_config.temperature = Some(temp);
    }
    if let Some(top_p) = request.top_p {
        gen_config.top_p = Some(top_p);
    }
    if let Some(top_k) = request.top_k {
        gen_config.top_k = Some(top_k);
    }
    if let Some(stop_seqs) = &request.stop_sequences {
        if !stop_seqs.is_empty() {
            gen_config.stop_sequences = Some(stop_seqs.clone());
        }
    }

    // Handle thinking configuration
    if is_thinking {
        let thinking_budget = request.thinking.as_ref().map(|t| t.budget_tokens);

        if is_claude {
            // Claude thinking config uses budget_tokens
            let thinking_config = GoogleThinkingConfig::claude(thinking_budget.unwrap_or(10000));

            // Validate max_tokens > thinking_budget
            if let Some(budget) = thinking_budget {
                if let Some(max) = gen_config.max_output_tokens {
                    if max <= budget {
                        // Bump max_tokens to allow for response content
                        gen_config.max_output_tokens = Some(budget + 8192);
                    }
                }
            }

            google_request.thinking_config = Some(thinking_config);
        } else if is_gemini {
            // Gemini thinking config uses thinkingBudget
            let thinking_config = GoogleThinkingConfig::gemini(thinking_budget.unwrap_or(16000));
            google_request.thinking_config = Some(thinking_config);
        }
    }

    // Cap max tokens for Gemini models
    if is_gemini {
        if let Some(max) = gen_config.max_output_tokens {
            if max > GEMINI_MAX_OUTPUT_TOKENS {
                gen_config.max_output_tokens = Some(GEMINI_MAX_OUTPUT_TOKENS);
            }
        }
    }

    google_request.generation_config = Some(gen_config);

    // Convert tools
    if let Some(tools) = &request.tools {
        if !tools.is_empty() {
            let declarations = convert_tools(tools);
            google_request.tools = Some(vec![GoogleTool::new(declarations)]);

            // For Claude models, set VALIDATED mode for strict parameter validation
            if is_claude {
                google_request.tool_config = Some(ToolConfig {
                    function_calling_config: crate::gemini::models::google::FunctionCallingConfig {
                        mode: "VALIDATED".to_string(),
                        allowed_function_names: None,
                    },
                });
            }
        }
    }

    // Handle tool_choice (convert to Google's tool_config)
    if let Some(tool_choice) = &request.tool_choice {
        google_request.tool_config = Some(convert_tool_choice(tool_choice));
    }

    google_request
}

/// Convert system prompt to Parts.
fn convert_system_prompt(system: &SystemPrompt) -> Vec<Part> {
    match system {
        SystemPrompt::Text(text) => {
            if text.is_empty() {
                vec![]
            } else {
                vec![Part::text(text)]
            }
        }
        SystemPrompt::Blocks(blocks) => blocks
            .iter()
            .filter_map(|block| {
                let crate::gemini::models::request::SystemBlock::Text { text, .. } = block;
                if text.is_empty() {
                    None
                } else {
                    Some(Part::text(text))
                }
            })
            .collect(),
    }
}

/// Convert messages to Google Content array.
fn convert_messages(messages: &[Message], model: &str) -> Vec<Content> {
    let mut contents = Vec::new();

    for msg in messages {
        let parts = convert_message_content(&msg.content, model);

        // Ensure at least one part (Google API requirement)
        let parts = if parts.is_empty() {
            // Use '.' as placeholder - empty text causes errors
            vec![Part::text(".")]
        } else {
            parts
        };

        contents.push(Content {
            role: Some(convert_role(msg.role)),
            parts,
        });
    }

    contents
}

/// Convert message content to Parts.
fn convert_message_content(content: &MessageContent, model: &str) -> Vec<Part> {
    match content {
        MessageContent::Text(text) => {
            if text.trim().is_empty() {
                vec![]
            } else {
                text_to_parts(text)
            }
        }
        MessageContent::Blocks(blocks) => convert_content_to_parts(blocks, model),
    }
}

/// Convert tools to FunctionDeclarations.
fn convert_tools(tools: &[crate::gemini::models::tools::Tool]) -> Vec<FunctionDeclaration> {
    tools
        .iter()
        .enumerate()
        .map(|(idx, tool)| {
            // Sanitize tool name (alphanumeric, underscore, hyphen only)
            let name = sanitize_tool_name(&tool.name, idx);

            // Sanitize schema for Google API
            let parameters = sanitize_schema(&tool.input_schema);

            FunctionDeclaration::new(name, tool.description.clone(), Some(parameters))
        })
        .collect()
}

/// Sanitize tool name for Google API compatibility.
fn sanitize_tool_name(name: &str, fallback_idx: usize) -> String {
    if name.is_empty() {
        return format!("tool_{}", fallback_idx);
    }

    let sanitized: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();

    // Limit length to 64 characters
    if sanitized.len() > 64 {
        sanitized[..64].to_string()
    } else {
        sanitized
    }
}

/// Convert tool choice to Google ToolConfig.
fn convert_tool_choice(choice: &crate::gemini::models::tools::ToolChoice) -> ToolConfig {
    match choice {
        crate::gemini::models::tools::ToolChoice::Auto => ToolConfig::auto(),
        crate::gemini::models::tools::ToolChoice::Any => ToolConfig::any(),
        crate::gemini::models::tools::ToolChoice::None => ToolConfig::none(),
        crate::gemini::models::tools::ToolChoice::Tool { name } => ToolConfig::force(name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gemini::models::tools::Tool;
    use serde_json::json;

    #[test]
    fn test_convert_simple_request() {
        let request = MessagesRequest::simple("claude-sonnet-4-5", 1024, "Hello!");
        let result = convert_request(&request);

        assert_eq!(result.contents.len(), 1);
        assert_eq!(result.contents[0].role, Some("user".to_string()));
        assert_eq!(
            result.generation_config.as_ref().unwrap().max_output_tokens,
            Some(1024)
        );
    }

    #[test]
    fn test_convert_with_system_prompt() {
        let request = MessagesRequest::builder()
            .model("claude-sonnet-4-5")
            .max_tokens(1024)
            .system("You are a helpful assistant.")
            .user_message("Hello!")
            .build();

        let result = convert_request(&request);

        assert!(result.system_instruction.is_some());
        let sys = result.system_instruction.as_ref().unwrap();
        assert_eq!(
            sys.parts[0].text,
            Some("You are a helpful assistant.".to_string())
        );
    }

    #[test]
    fn test_convert_with_temperature() {
        let request = MessagesRequest::builder()
            .model("claude-sonnet-4-5")
            .max_tokens(1024)
            .temperature(0.7)
            .user_message("Hello!")
            .build();

        let result = convert_request(&request);

        let gen_config = result.generation_config.as_ref().unwrap();
        assert!((gen_config.temperature.unwrap() - 0.7).abs() < 0.001);
    }

    #[test]
    fn test_convert_with_tools() {
        let tool = Tool::new(
            "get_weather",
            "Get the weather",
            json!({
                "type": "object",
                "properties": {
                    "location": { "type": "string" }
                }
            }),
        );

        let request = MessagesRequest::builder()
            .model("claude-sonnet-4-5")
            .max_tokens(1024)
            .tool(tool)
            .user_message("What's the weather?")
            .build();

        let result = convert_request(&request);

        assert!(result.tools.is_some());
        let tools = result.tools.as_ref().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].function_declarations.len(), 1);
        assert_eq!(tools[0].function_declarations[0].name, "get_weather");
    }

    #[test]
    fn test_convert_claude_thinking() {
        let request = MessagesRequest::builder()
            .model("claude-sonnet-4-5-thinking")
            .max_tokens(2048)
            .thinking(10000)
            .user_message("Hello!")
            .build();

        let result = convert_request(&request);

        assert!(result.thinking_config.is_some());
        let thinking = result.thinking_config.as_ref().unwrap();
        assert_eq!(thinking.budget_tokens, Some(10000));
    }

    #[test]
    fn test_convert_gemini_thinking() {
        let request = MessagesRequest::builder()
            .model("gemini-3-flash")
            .max_tokens(2048)
            .thinking(8000)
            .user_message("Hello!")
            .build();

        let result = convert_request(&request);

        assert!(result.thinking_config.is_some());
        let thinking = result.thinking_config.as_ref().unwrap();
        assert_eq!(thinking.include_thoughts, Some(true));
        assert_eq!(thinking.thinking_budget, Some(8000));
    }

    #[test]
    fn test_gemini_max_tokens_cap() {
        let request = MessagesRequest::builder()
            .model("gemini-3-flash")
            .max_tokens(100000) // Above limit
            .user_message("Hello!")
            .build();

        let result = convert_request(&request);

        assert_eq!(
            result.generation_config.as_ref().unwrap().max_output_tokens,
            Some(GEMINI_MAX_OUTPUT_TOKENS)
        );
    }

    #[test]
    fn test_claude_thinking_with_tools_hint() {
        let tool = Tool::new("test_tool", "Test", json!({"type": "object"}));

        let request = MessagesRequest::builder()
            .model("claude-sonnet-4-5-thinking")
            .max_tokens(2048)
            .thinking(10000)
            .tool(tool)
            .system("You are helpful.")
            .user_message("Hello!")
            .build();

        let result = convert_request(&request);

        let sys = result.system_instruction.as_ref().unwrap();
        let text = sys.parts[0].text.as_ref().unwrap();
        assert!(text.contains("Interleaved thinking"));
    }

    #[test]
    fn test_convert_tool_choice_auto() {
        let config = convert_tool_choice(&crate::gemini::models::tools::ToolChoice::Auto);
        assert_eq!(config.function_calling_config.mode, "AUTO");
    }

    #[test]
    fn test_convert_tool_choice_any() {
        let config = convert_tool_choice(&crate::gemini::models::tools::ToolChoice::Any);
        assert_eq!(config.function_calling_config.mode, "ANY");
    }

    #[test]
    fn test_convert_tool_choice_none() {
        let config = convert_tool_choice(&crate::gemini::models::tools::ToolChoice::None);
        assert_eq!(config.function_calling_config.mode, "NONE");
    }

    #[test]
    fn test_convert_tool_choice_specific() {
        let config = convert_tool_choice(&crate::gemini::models::tools::ToolChoice::tool("my_tool"));
        assert_eq!(config.function_calling_config.mode, "ANY");
        assert_eq!(
            config.function_calling_config.allowed_function_names,
            Some(vec!["my_tool".to_string()])
        );
    }

    #[test]
    fn test_sanitize_tool_name() {
        // Normal name
        assert_eq!(sanitize_tool_name("get_weather", 0), "get_weather");

        // Special characters replaced
        assert_eq!(sanitize_tool_name("get.weather!", 0), "get_weather_");

        // Empty name gets fallback
        assert_eq!(sanitize_tool_name("", 5), "tool_5");

        // Long name is truncated
        let long_name = "a".repeat(100);
        let sanitized = sanitize_tool_name(&long_name, 0);
        assert_eq!(sanitized.len(), 64);
    }

    #[test]
    fn test_convert_empty_messages() {
        let request = MessagesRequest::builder()
            .model("claude-sonnet-4-5")
            .max_tokens(1024)
            .user_message("") // Empty message
            .build();

        let result = convert_request(&request);

        // Should have placeholder content
        assert_eq!(result.contents.len(), 1);
        assert_eq!(result.contents[0].parts[0].text, Some(".".to_string()));
    }

    #[test]
    fn test_convert_stop_sequences() {
        let request = MessagesRequest::builder()
            .model("claude-sonnet-4-5")
            .max_tokens(1024)
            .stop_sequences(vec!["END".to_string(), "STOP".to_string()])
            .user_message("Hello!")
            .build();

        let result = convert_request(&request);

        let gen_config = result.generation_config.as_ref().unwrap();
        assert_eq!(
            gen_config.stop_sequences,
            Some(vec!["END".to_string(), "STOP".to_string()])
        );
    }

    #[test]
    fn test_convert_multi_turn_conversation() {
        let request = MessagesRequest::builder()
            .model("claude-sonnet-4-5")
            .max_tokens(1024)
            .user_message("Hello!")
            .assistant_message("Hi there!")
            .user_message("How are you?")
            .build();

        let result = convert_request(&request);

        assert_eq!(result.contents.len(), 3);
        assert_eq!(result.contents[0].role, Some("user".to_string()));
        assert_eq!(result.contents[1].role, Some("model".to_string()));
        assert_eq!(result.contents[2].role, Some("user".to_string()));
    }

    #[test]
    fn test_thinking_budget_adjustment() {
        // When max_tokens <= thinking_budget, max_tokens should be adjusted
        let request = MessagesRequest::builder()
            .model("claude-sonnet-4-5-thinking")
            .max_tokens(5000) // Less than thinking budget
            .thinking(10000)
            .user_message("Hello!")
            .build();

        let result = convert_request(&request);

        let gen_config = result.generation_config.as_ref().unwrap();
        // Should be adjusted to budget + 8192
        assert_eq!(gen_config.max_output_tokens, Some(10000 + 8192));
    }

    #[test]
    fn test_claude_validated_tool_mode() {
        let tool = Tool::new("test", "Test tool", json!({"type": "object"}));

        let request = MessagesRequest::builder()
            .model("claude-sonnet-4-5")
            .max_tokens(1024)
            .tool(tool)
            .user_message("Hello!")
            .build();

        let result = convert_request(&request);

        // Claude should use VALIDATED mode
        assert!(result.tool_config.is_some());
        assert_eq!(
            result
                .tool_config
                .as_ref()
                .unwrap()
                .function_calling_config
                .mode,
            "VALIDATED"
        );
    }

    #[test]
    fn test_convert_system_blocks() {
        use crate::gemini::models::request::SystemBlock;

        let request = MessagesRequest::builder()
            .model("claude-sonnet-4-5")
            .max_tokens(1024)
            .system_blocks(vec![
                SystemBlock::text("Part 1"),
                SystemBlock::text("Part 2"),
            ])
            .user_message("Hello!")
            .build();

        let result = convert_request(&request);

        assert!(result.system_instruction.is_some());
        let sys = result.system_instruction.as_ref().unwrap();
        assert_eq!(sys.parts.len(), 2);
    }
}
