//! Content block conversion between Anthropic and Google formats.
//!
//! This module provides bidirectional conversion between Anthropic's `ContentBlock`
//! types and Google's `Part` format used by the Generative AI API.
//!
//! ## Anthropic to Google Conversion
//!
//! | Anthropic Type | Google Type |
//! |----------------|-------------|
//! | Text | `{ text: "..." }` |
//! | ToolUse | `{ functionCall: { name, args, id? } }` |
//! | ToolResult | `{ functionResponse: { name, response, id? } }` |
//! | Thinking | `{ text, thought: true, thoughtSignature? }` |
//! | Image (base64) | `{ inlineData: { mimeType, data } }` |
//! | Document | `{ inlineData: { mimeType, data } }` |
//!
//! ## Role Mapping
//!
//! | Anthropic | Google |
//! |-----------|--------|
//! | user | user |
//! | assistant | model |

use serde_json::json;

use crate::providers::gemini::constants::{
    GEMINI_SKIP_SIGNATURE, MIN_SIGNATURE_LENGTH, ModelFamily, get_model_family,
};
use crate::providers::gemini::models::content::{ContentBlock, ImageSource, ToolResultContent};
use crate::providers::gemini::models::google::{
    Content, FunctionCall, FunctionResponse, InlineData, Part,
};
use crate::providers::gemini::models::request::Role;

use crate::providers::gemini::thinking::GLOBAL_SIGNATURE_CACHE;

/// Convert an Anthropic role to a Google role.
///
/// - `user` -> `user`
/// - `assistant` -> `model`
pub fn convert_role(role: Role) -> String {
    match role {
        Role::User => "user".to_string(),
        Role::Assistant => "model".to_string(),
    }
}

/// Convert a Google role string back to Anthropic role.
///
/// - `user` -> `User`
/// - `model` -> `Assistant`
/// - other -> `User` (fallback)
#[allow(dead_code)]
pub fn google_role_to_anthropic(role: &str) -> Role {
    match role {
        "model" => Role::Assistant,
        "user" => Role::User,
        _ => Role::User,
    }
}

/// Convert Anthropic message content to Google Parts.
///
/// Handles all content block types and performs necessary transformations
/// for model-specific requirements.
///
/// # Arguments
///
/// * `content` - The message content (can be text string or block array)
/// * `model` - The target model name (for family detection)
///
/// # Returns
///
/// A vector of Google `Part` objects.
pub(crate) fn convert_content_to_parts(content: &[ContentBlock], model: &str) -> Vec<Part> {
    let model_family = get_model_family(model);
    let is_claude = model_family == ModelFamily::Claude;
    let is_gemini = model_family == ModelFamily::Gemini;

    let mut parts = Vec::new();
    let mut deferred_inline_data = Vec::new();

    for block in content {
        match block {
            ContentBlock::Text { text } => {
                // Skip empty text blocks - they cause API errors
                if !text.trim().is_empty() {
                    parts.push(Part::text(text));
                }
            }

            ContentBlock::ToolUse { id, name, input } => {
                let mut call = FunctionCall::new(name, input.clone());

                // For Claude models, include the id field
                if is_claude {
                    call.id = Some(id.clone());
                }

                let mut part = Part::function_call(call);

                // For Gemini models, include thoughtSignature at the part level
                if is_gemini {
                    let signature = GLOBAL_SIGNATURE_CACHE
                        .get_tool_signature(id)
                        .unwrap_or_else(|| GEMINI_SKIP_SIGNATURE.to_string());
                    part.thought_signature = Some(signature);
                }

                parts.push(part);
            }

            ContentBlock::ToolResult {
                tool_use_id,
                content: result_content,
                is_error,
            } => {
                let (response_text, mut image_parts) =
                    extract_tool_result_content(result_content, *is_error);

                let mut response = FunctionResponse::new(tool_use_id, response_text);

                // For Claude models, the id field must match the tool_use_id
                if is_claude {
                    response.id = Some(tool_use_id.clone());
                }

                parts.push(Part::function_response(response));

                // Defer images to end of parts array to ensure functionResponse
                // parts are consecutive (required by Claude's API)
                deferred_inline_data.append(&mut image_parts);
            }

            ContentBlock::Thinking {
                thinking,
                signature,
            } => {
                // Handle thinking blocks with signature compatibility check
                if let Some(sig) = signature {
                    if sig.len() >= MIN_SIGNATURE_LENGTH {
                        // Check signature compatibility for Gemini
                        if is_gemini {
                            let is_compatible = GLOBAL_SIGNATURE_CACHE
                                .is_signature_compatible(sig, ModelFamily::Gemini);

                            if !is_compatible {
                                // Drop incompatible signatures for Gemini
                                continue;
                            }
                        }

                        // Compatible - convert to Google format with signature
                        parts.push(Part::thought(thinking, Some(sig.clone())));
                    }
                }
                // Unsigned thinking blocks are dropped (existing behavior)
            }

            ContentBlock::Image { source } => {
                match source {
                    ImageSource::Base64 { media_type, data } => {
                        parts.push(Part::inline_data(InlineData::new(media_type, data)));
                    }
                    ImageSource::Url { url: _ } => {
                        // URL images not directly supported - would need fileData
                        // For now, skip (in production, you might
                        // want to fetch and convert to base64)
                    }
                }
            }

            ContentBlock::Document { source } => {
                let media_type = source.media_type();
                let data = source.data();
                parts.push(Part::inline_data(InlineData::new(media_type, data)));
            }
        }
    }

    // Add deferred inline data at the end
    parts.extend(deferred_inline_data);

    parts
}

/// Extract content and images from a tool result.
///
/// Returns (text_content, image_parts) where text_content is the JSON-encoded
/// result and image_parts are any images that need to be appended.
fn extract_tool_result_content(
    content: &Option<ToolResultContent>,
    is_error: Option<bool>,
) -> (String, Vec<Part>) {
    let is_error = is_error.unwrap_or(false);

    match content {
        None => {
            let result = if is_error {
                json!({ "error": "" })
            } else {
                json!({ "result": "" })
            };
            (result.to_string(), vec![])
        }
        Some(ToolResultContent::Text(text)) => {
            let result = if is_error {
                json!({ "error": text })
            } else {
                json!({ "result": text })
            };
            (result.to_string(), vec![])
        }
        Some(ToolResultContent::Blocks(blocks)) => {
            let mut texts = Vec::new();
            let mut image_parts = Vec::new();

            for block in blocks {
                match block {
                    ContentBlock::Text { text } => {
                        texts.push(text.clone());
                    }
                    ContentBlock::Image {
                        source: ImageSource::Base64 { media_type, data },
                    } => {
                        image_parts.push(Part::inline_data(InlineData::new(media_type, data)));
                    }
                    _ => {}
                }
            }

            let text_content = texts.join("\n");
            let has_images = !image_parts.is_empty();

            let result = if is_error {
                json!({ "error": text_content })
            } else if text_content.is_empty() && has_images {
                json!({ "result": "Image attached" })
            } else {
                json!({ "result": text_content })
            };

            (result.to_string(), image_parts)
        }
    }
}

/// Convert Google Parts back to Anthropic ContentBlocks.
///
/// Used when converting Google API responses to Anthropic format.
///
/// # Arguments
///
/// * `parts` - The Google Parts to convert
/// * `model` - The model name (for caching signatures)
///
/// # Returns
///
/// A vector of Anthropic `ContentBlock` objects.
pub(crate) fn convert_parts_to_content(parts: &[Part], model: &str) -> Vec<ContentBlock> {
    let model_family = get_model_family(model);
    let mut blocks = Vec::new();

    for part in parts {
        // Text content (including thoughts)
        if let Some(text) = &part.text {
            if part.is_thought() {
                let signature = part.thought_signature.clone().unwrap_or_default();

                // Cache thinking signature with model family
                if signature.len() >= MIN_SIGNATURE_LENGTH {
                    GLOBAL_SIGNATURE_CACHE.store_thinking_signature(text, &signature, model_family);
                }

                blocks.push(ContentBlock::Thinking {
                    thinking: text.clone(),
                    signature: if signature.is_empty() {
                        None
                    } else {
                        Some(signature)
                    },
                });
            } else {
                blocks.push(ContentBlock::Text { text: text.clone() });
            }
        }

        // Function calls
        if let Some(fc) = &part.function_call {
            let tool_id = fc.id.clone().unwrap_or_else(generate_tool_use_id);

            // Cache the signature for this tool_use_id
            if let Some(sig) = &part.thought_signature {
                if sig.len() >= MIN_SIGNATURE_LENGTH {
                    GLOBAL_SIGNATURE_CACHE.store_tool_signature(&tool_id, sig, model_family);
                }
            }

            blocks.push(ContentBlock::ToolUse {
                id: tool_id,
                name: fc.name.clone(),
                input: fc.args.clone(),
            });
        }

        // Inline data (images)
        if let Some(inline) = &part.inline_data {
            blocks.push(ContentBlock::Image {
                source: ImageSource::Base64 {
                    media_type: inline.mime_type.clone(),
                    data: inline.data.clone(),
                },
            });
        }
    }

    blocks
}

/// Generate a unique tool_use_id in the format `toolu_{hex}`.
fn generate_tool_use_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("toolu_{:024x}", timestamp)
}

/// Convert a simple text string to Parts.
pub fn text_to_parts(text: &str) -> Vec<Part> {
    vec![Part::text(text)]
}

/// Create a Google Content object from role and parts.
#[allow(dead_code)]
pub fn create_content(role: Role, parts: Vec<Part>) -> Content {
    Content {
        role: Some(convert_role(role)),
        parts,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_convert_role() {
        assert_eq!(convert_role(Role::User), "user");
        assert_eq!(convert_role(Role::Assistant), "model");
    }

    #[test]
    fn test_google_role_to_anthropic() {
        assert_eq!(google_role_to_anthropic("user"), Role::User);
        assert_eq!(google_role_to_anthropic("model"), Role::Assistant);
        assert_eq!(google_role_to_anthropic("unknown"), Role::User);
    }

    #[test]
    fn test_convert_text_block() {
        let blocks = vec![ContentBlock::text("Hello, world!")];
        let parts = convert_content_to_parts(&blocks, "claude-sonnet-4-5");

        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].text, Some("Hello, world!".to_string()));
    }

    #[test]
    fn test_skip_empty_text_blocks() {
        let blocks = vec![
            ContentBlock::text(""),
            ContentBlock::text("   "),
            ContentBlock::text("Hello"),
        ];
        let parts = convert_content_to_parts(&blocks, "claude-sonnet-4-5");

        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].text, Some("Hello".to_string()));
    }

    #[test]
    fn test_convert_tool_use_claude() {
        let blocks = vec![ContentBlock::tool_use(
            "toolu_123",
            "get_weather",
            json!({"location": "NYC"}),
        )];
        let parts = convert_content_to_parts(&blocks, "claude-sonnet-4-5");

        assert_eq!(parts.len(), 1);
        let fc = parts[0].function_call.as_ref().unwrap();
        assert_eq!(fc.name, "get_weather");
        assert_eq!(fc.args["location"], "NYC");
        assert_eq!(fc.id, Some("toolu_123".to_string()));
        // Claude doesn't get thoughtSignature
        assert!(parts[0].thought_signature.is_none());
    }

    #[test]
    fn test_convert_tool_use_gemini() {
        // Clear cache first
        GLOBAL_SIGNATURE_CACHE.clear_all();

        let blocks = vec![ContentBlock::tool_use(
            "toolu_456",
            "search",
            json!({"query": "rust"}),
        )];
        let parts = convert_content_to_parts(&blocks, "gemini-3-flash");

        assert_eq!(parts.len(), 1);
        let fc = parts[0].function_call.as_ref().unwrap();
        assert_eq!(fc.name, "search");
        // Gemini doesn't get id
        assert!(fc.id.is_none());
        // Gemini gets thoughtSignature (sentinel since no cache)
        assert_eq!(
            parts[0].thought_signature,
            Some(GEMINI_SKIP_SIGNATURE.to_string())
        );
    }

    #[test]
    fn test_convert_tool_result_text() {
        let blocks = vec![ContentBlock::tool_result("toolu_123", "Sunny, 72F")];
        let parts = convert_content_to_parts(&blocks, "claude-sonnet-4-5");

        assert_eq!(parts.len(), 1);
        let fr = parts[0].function_response.as_ref().unwrap();
        assert_eq!(fr.name, "toolu_123");
        assert!(fr.response.content.contains("Sunny, 72F"));
    }

    #[test]
    fn test_convert_tool_result_error() {
        let blocks = vec![ContentBlock::tool_result_error("toolu_123", "API failed")];
        let parts = convert_content_to_parts(&blocks, "claude-sonnet-4-5");

        assert_eq!(parts.len(), 1);
        let fr = parts[0].function_response.as_ref().unwrap();
        assert!(fr.response.content.contains("error"));
        assert!(fr.response.content.contains("API failed"));
    }

    #[test]
    fn test_convert_image_base64() {
        let blocks = vec![ContentBlock::image_base64("image/png", "iVBORw0KGgo=")];
        let parts = convert_content_to_parts(&blocks, "claude-sonnet-4-5");

        assert_eq!(parts.len(), 1);
        let inline = parts[0].inline_data.as_ref().unwrap();
        assert_eq!(inline.mime_type, "image/png");
        assert_eq!(inline.data, "iVBORw0KGgo=");
    }

    #[test]
    fn test_convert_document() {
        let blocks = vec![ContentBlock::document_base64("application/pdf", "JVBERi0=")];
        let parts = convert_content_to_parts(&blocks, "claude-sonnet-4-5");

        assert_eq!(parts.len(), 1);
        let inline = parts[0].inline_data.as_ref().unwrap();
        assert_eq!(inline.mime_type, "application/pdf");
        assert_eq!(inline.data, "JVBERi0=");
    }

    #[test]
    fn test_convert_thinking_with_signature() {
        // First, store a compatible signature in cache
        GLOBAL_SIGNATURE_CACHE.store_thinking_signature(
            "Let me think about this...",
            "x".repeat(100),
            ModelFamily::Gemini,
        );

        let blocks = vec![ContentBlock::thinking(
            "Let me think about this...",
            Some("x".repeat(100)),
        )];
        let parts = convert_content_to_parts(&blocks, "gemini-3-flash");

        assert_eq!(parts.len(), 1);
        assert!(parts[0].thought.unwrap_or(false));
        assert_eq!(
            parts[0].text,
            Some("Let me think about this...".to_string())
        );
        assert_eq!(parts[0].thought_signature, Some("x".repeat(100)));
    }

    #[test]
    fn test_convert_thinking_drops_unsigned() {
        let blocks = vec![ContentBlock::thinking("Thinking without signature", None)];
        let parts = convert_content_to_parts(&blocks, "gemini-3-flash");

        // Unsigned thinking blocks are dropped
        assert!(parts.is_empty());
    }

    #[test]
    fn test_convert_parts_to_content_text() {
        let parts = vec![Part::text("Hello from Google!")];
        let blocks = convert_parts_to_content(&parts, "claude-sonnet-4-5");

        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].is_text());
        assert_eq!(blocks[0].as_text(), Some("Hello from Google!"));
    }

    #[test]
    fn test_convert_parts_to_content_function_call() {
        let call = FunctionCall::with_id("search", json!({"q": "test"}), "toolu_999");
        let parts = vec![Part::function_call(call)];
        let blocks = convert_parts_to_content(&parts, "claude-sonnet-4-5");

        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].is_tool_use());
        let (id, name, input) = blocks[0].as_tool_use().unwrap();
        assert_eq!(id, "toolu_999");
        assert_eq!(name, "search");
        assert_eq!(input["q"], "test");
    }

    #[test]
    fn test_convert_parts_to_content_thought() {
        let parts = vec![Part::thought("Analyzing...", Some("sig123".to_string()))];
        let blocks = convert_parts_to_content(&parts, "gemini-3-flash");

        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].is_thinking());
    }

    #[test]
    fn test_convert_parts_to_content_inline_data() {
        let data = InlineData::new("image/jpeg", "base64data");
        let parts = vec![Part::inline_data(data)];
        let blocks = convert_parts_to_content(&parts, "claude-sonnet-4-5");

        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].is_image());
    }

    #[test]
    fn test_generate_tool_use_id() {
        let id1 = generate_tool_use_id();
        let id2 = generate_tool_use_id();

        assert!(id1.starts_with("toolu_"));
        assert!(id2.starts_with("toolu_"));
        assert_ne!(id1, id2); // Should be unique
    }

    #[test]
    fn test_text_to_parts() {
        let parts = text_to_parts("Simple text");
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].text, Some("Simple text".to_string()));
    }

    #[test]
    fn test_create_content() {
        let parts = vec![Part::text("Test")];
        let content = create_content(Role::User, parts);

        assert_eq!(content.role, Some("user".to_string()));
        assert_eq!(content.parts.len(), 1);
    }

    #[test]
    fn test_extract_tool_result_content_text() {
        let content = Some(ToolResultContent::text("Result text"));
        let (text, images) = extract_tool_result_content(&content, Some(false));

        assert!(text.contains("Result text"));
        assert!(text.contains("result"));
        assert!(images.is_empty());
    }

    #[test]
    fn test_extract_tool_result_content_error() {
        let content = Some(ToolResultContent::text("Error message"));
        let (text, images) = extract_tool_result_content(&content, Some(true));

        assert!(text.contains("Error message"));
        assert!(text.contains("error"));
        assert!(images.is_empty());
    }

    #[test]
    fn test_extract_tool_result_content_none() {
        let (text, images) = extract_tool_result_content(&None, Some(false));

        assert!(text.contains("result"));
        assert!(images.is_empty());
    }

    #[test]
    fn test_tool_result_with_images() {
        let blocks = vec![
            ContentBlock::text("Here's the image:"),
            ContentBlock::image_base64("image/png", "imgdata"),
        ];
        let content = Some(ToolResultContent::Blocks(blocks));
        let (text, images) = extract_tool_result_content(&content, Some(false));

        assert!(text.contains("Here's the image:"));
        assert_eq!(images.len(), 1);
    }

    #[test]
    fn test_roundtrip_text() {
        let original = vec![ContentBlock::text("Hello, roundtrip!")];
        let parts = convert_content_to_parts(&original, "claude-sonnet-4-5");
        let result = convert_parts_to_content(&parts, "claude-sonnet-4-5");

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].as_text(), original[0].as_text());
    }
}
