//! Response conversion from Google to Anthropic format.
//!
//! This module converts `GoogleResponse` (Google Generative AI format) to
//! `MessagesResponse` (Anthropic format) for return to clients.
//!
//! ## Conversion Details
//!
//! ### Content
//! - Parts are converted to ContentBlocks (see `content` module)
//! - Function calls become `tool_use` blocks with generated IDs
//! - Thought parts become `thinking` blocks
//!
//! ### Finish Reasons
//! - `STOP` -> `end_turn`
//! - `MAX_TOKENS` -> `max_tokens`
//! - `TOOL_USE` or function calls -> `tool_use`
//! - `SAFETY` -> `end_turn` (with safety flag)
//!
//! ### Usage Metadata
//! - `promptTokenCount` -> `input_tokens` (minus cached)
//! - `candidatesTokenCount` -> `output_tokens`
//! - `cachedContentTokenCount` -> `cache_read_input_tokens`

use std::time::{SystemTime, UNIX_EPOCH};

use crate::providers::gemini::models::content::ContentBlock;
use crate::providers::gemini::models::google::GoogleResponse;
use crate::providers::gemini::models::response::{MessagesResponse, StopReason, Usage};

use super::content::convert_parts_to_content;

/// Convert a Google `GoogleResponse` to an Anthropic `MessagesResponse`.
///
/// This is the main entry point for response conversion. It handles the
/// candidate extraction, content conversion, and usage calculation.
///
/// # Arguments
///
/// * `response` - The Google Generative AI API response
/// * `model` - The model name (used for caching signatures)
///
/// # Returns
///
/// An Anthropic `MessagesResponse` suitable for returning to clients.
///
/// # Example
///
/// ```rust,ignore
/// use gaud::providers::gemini::convert::convert_response;
///
/// let google_response = /* ... */;
/// let anthropic_response = convert_response(&google_response, "claude-sonnet-4-5");
/// ```
pub fn convert_response(response: &GoogleResponse, model: &str) -> MessagesResponse {
    // Extract first candidate (Google may return multiple)
    let candidate = response.candidates.first();

    // Get parts from candidate content
    let parts = candidate
        .and_then(|c| c.content.as_ref())
        .map(|content| content.parts.as_slice())
        .unwrap_or(&[]);

    // Convert parts to Anthropic content blocks
    let mut content = convert_parts_to_content(parts, model);

    // Ensure at least one content block (API requirement)
    if content.is_empty() {
        content.push(ContentBlock::text(""));
    }

    // Determine stop reason
    let finish_reason = candidate.and_then(|c| c.finish_reason.as_deref());
    let has_tool_calls = content.iter().any(|b| b.is_tool_use());
    let stop_reason = determine_stop_reason(finish_reason, has_tool_calls);

    // Extract usage metadata
    let usage = extract_usage(response);

    // Generate message ID
    let id = generate_message_id();

    MessagesResponse {
        id,
        response_type: "message".to_string(),
        role: crate::providers::gemini::models::request::Role::Assistant,
        content,
        model: model.to_string(),
        stop_reason: Some(stop_reason),
        stop_sequence: None,
        usage,
    }
}

/// Determine the stop reason from Google's finish reason.
fn determine_stop_reason(finish_reason: Option<&str>, has_tool_calls: bool) -> StopReason {
    match finish_reason {
        Some("STOP") => StopReason::EndTurn,
        Some("MAX_TOKENS") => StopReason::MaxTokens,
        Some("TOOL_USE") => StopReason::ToolUse,
        Some("SAFETY") => StopReason::EndTurn, // Could add safety-specific handling
        Some("RECITATION") => StopReason::EndTurn,
        Some("OTHER") => StopReason::EndTurn,
        None if has_tool_calls => StopReason::ToolUse,
        _ => StopReason::EndTurn,
    }
}

/// Extract usage metadata from the response.
fn extract_usage(response: &GoogleResponse) -> Usage {
    let metadata = response.usage_metadata.as_ref();

    let prompt_tokens = metadata.map(|m| m.prompt_token_count).unwrap_or(0);
    let cached_tokens = metadata
        .and_then(|m| m.cached_content_token_count)
        .unwrap_or(0);
    let output_tokens = metadata.map(|m| m.candidates_token_count).unwrap_or(0);

    // Anthropic's input_tokens excludes cached, so we subtract
    let input_tokens = prompt_tokens.saturating_sub(cached_tokens);

    Usage {
        input_tokens,
        output_tokens,
        cache_read_input_tokens: if cached_tokens > 0 {
            Some(cached_tokens)
        } else {
            None
        },
        cache_creation_input_tokens: None,
    }
}

/// Generate a unique message ID in the format `msg_{uuid}`.
fn generate_message_id() -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();

    // Use timestamp + random-ish data for uniqueness
    format!("msg_{:032x}", timestamp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::gemini::models::google::{
        Candidate, Content, FunctionCall, Part, UsageMetadata,
    };
    use serde_json::json;

    fn create_text_response(text: &str, finish_reason: &str) -> GoogleResponse {
        GoogleResponse {
            candidates: vec![Candidate {
                content: Some(Content {
                    role: Some("model".to_string()),
                    parts: vec![Part::text(text)],
                }),
                finish_reason: Some(finish_reason.to_string()),
                safety_ratings: vec![],
                citation_metadata: None,
                index: None,
            }],
            usage_metadata: Some(UsageMetadata {
                prompt_token_count: 100,
                candidates_token_count: 50,
                cached_content_token_count: Some(10),
                total_token_count: 150,
                thoughts_token_count: None,
            }),
            model_version: None,
        }
    }

    #[test]
    fn test_convert_simple_text_response() {
        let google_response = create_text_response("Hello, world!", "STOP");
        let result = convert_response(&google_response, "claude-sonnet-4-5");

        assert!(result.id.starts_with("msg_"));
        assert_eq!(result.response_type, "message");
        assert_eq!(
            result.role,
            crate::providers::gemini::models::request::Role::Assistant
        );
        assert_eq!(result.content.len(), 1);
        assert_eq!(result.content[0].as_text(), Some("Hello, world!"));
        assert_eq!(result.stop_reason, Some(StopReason::EndTurn));
        assert_eq!(result.model, "claude-sonnet-4-5");
    }

    #[test]
    fn test_convert_stop_reasons() {
        let cases = vec![
            ("STOP", StopReason::EndTurn),
            ("MAX_TOKENS", StopReason::MaxTokens),
            ("TOOL_USE", StopReason::ToolUse),
            ("SAFETY", StopReason::EndTurn),
            ("OTHER", StopReason::EndTurn),
        ];

        for (reason, expected) in cases {
            let google_response = create_text_response("Test", reason);
            let result = convert_response(&google_response, "claude-sonnet-4-5");
            assert_eq!(
                result.stop_reason,
                Some(expected),
                "Finish reason {} should map to {:?}",
                reason,
                expected
            );
        }
    }

    #[test]
    fn test_convert_function_call_response() {
        let call = FunctionCall::with_id("get_weather", json!({"location": "NYC"}), "toolu_123");
        let google_response = GoogleResponse {
            candidates: vec![Candidate {
                content: Some(Content {
                    role: Some("model".to_string()),
                    parts: vec![Part::function_call(call)],
                }),
                finish_reason: Some("TOOL_USE".to_string()),
                safety_ratings: vec![],
                citation_metadata: None,
                index: None,
            }],
            usage_metadata: None,
            model_version: None,
        };

        let result = convert_response(&google_response, "claude-sonnet-4-5");

        assert_eq!(result.content.len(), 1);
        assert!(result.content[0].is_tool_use());
        let (id, name, input) = result.content[0].as_tool_use().unwrap();
        assert_eq!(id, "toolu_123");
        assert_eq!(name, "get_weather");
        assert_eq!(input["location"], "NYC");
        assert_eq!(result.stop_reason, Some(StopReason::ToolUse));
    }

    #[test]
    fn test_tool_use_inferred_from_content() {
        // No finish_reason, but has function call -> should infer tool_use
        let call = FunctionCall::new("search", json!({}));
        let google_response = GoogleResponse {
            candidates: vec![Candidate {
                content: Some(Content {
                    role: Some("model".to_string()),
                    parts: vec![Part::function_call(call)],
                }),
                finish_reason: None, // Not specified
                safety_ratings: vec![],
                citation_metadata: None,
                index: None,
            }],
            usage_metadata: None,
            model_version: None,
        };

        let result = convert_response(&google_response, "claude-sonnet-4-5");

        assert_eq!(result.stop_reason, Some(StopReason::ToolUse));
    }

    #[test]
    fn test_convert_usage_metadata() {
        let google_response = GoogleResponse {
            candidates: vec![Candidate {
                content: Some(Content {
                    role: Some("model".to_string()),
                    parts: vec![Part::text("Test")],
                }),
                finish_reason: Some("STOP".to_string()),
                safety_ratings: vec![],
                citation_metadata: None,
                index: None,
            }],
            usage_metadata: Some(UsageMetadata {
                prompt_token_count: 1000,
                candidates_token_count: 200,
                cached_content_token_count: Some(300),
                total_token_count: 1200,
                thoughts_token_count: None,
            }),
            model_version: None,
        };

        let result = convert_response(&google_response, "claude-sonnet-4-5");

        // input_tokens = prompt - cached = 1000 - 300 = 700
        assert_eq!(result.usage.input_tokens, 700);
        assert_eq!(result.usage.output_tokens, 200);
        assert_eq!(result.usage.cache_read_input_tokens, Some(300));
    }

    #[test]
    fn test_convert_no_usage_metadata() {
        let google_response = GoogleResponse {
            candidates: vec![Candidate {
                content: Some(Content {
                    role: Some("model".to_string()),
                    parts: vec![Part::text("Test")],
                }),
                finish_reason: Some("STOP".to_string()),
                safety_ratings: vec![],
                citation_metadata: None,
                index: None,
            }],
            usage_metadata: None,
            model_version: None,
        };

        let result = convert_response(&google_response, "claude-sonnet-4-5");

        assert_eq!(result.usage.input_tokens, 0);
        assert_eq!(result.usage.output_tokens, 0);
        assert!(result.usage.cache_read_input_tokens.is_none());
    }

    #[test]
    fn test_convert_empty_response() {
        let google_response = GoogleResponse {
            candidates: vec![Candidate {
                content: Some(Content {
                    role: Some("model".to_string()),
                    parts: vec![],
                }),
                finish_reason: Some("STOP".to_string()),
                safety_ratings: vec![],
                citation_metadata: None,
                index: None,
            }],
            usage_metadata: None,
            model_version: None,
        };

        let result = convert_response(&google_response, "claude-sonnet-4-5");

        // Should have placeholder content
        assert_eq!(result.content.len(), 1);
        assert!(result.content[0].is_text());
    }

    #[test]
    fn test_convert_no_candidates() {
        let google_response = GoogleResponse {
            candidates: vec![],
            usage_metadata: None,
            model_version: None,
        };

        let result = convert_response(&google_response, "claude-sonnet-4-5");

        // Should have placeholder content
        assert_eq!(result.content.len(), 1);
        assert_eq!(result.stop_reason, Some(StopReason::EndTurn));
    }

    #[test]
    fn test_convert_thinking_response() {
        let google_response = GoogleResponse {
            candidates: vec![Candidate {
                content: Some(Content {
                    role: Some("model".to_string()),
                    parts: vec![
                        Part::thought("Let me think about this...", Some("sig123".to_string())),
                        Part::text("Here's my answer."),
                    ],
                }),
                finish_reason: Some("STOP".to_string()),
                safety_ratings: vec![],
                citation_metadata: None,
                index: None,
            }],
            usage_metadata: None,
            model_version: None,
        };

        let result = convert_response(&google_response, "gemini-3-flash");

        assert_eq!(result.content.len(), 2);
        assert!(result.content[0].is_thinking());
        assert!(result.content[1].is_text());
    }

    #[test]
    fn test_convert_mixed_content() {
        let call = FunctionCall::new("tool1", json!({}));
        let google_response = GoogleResponse {
            candidates: vec![Candidate {
                content: Some(Content {
                    role: Some("model".to_string()),
                    parts: vec![
                        Part::text("I'll help with that."),
                        Part::function_call(call),
                    ],
                }),
                finish_reason: Some("TOOL_USE".to_string()),
                safety_ratings: vec![],
                citation_metadata: None,
                index: None,
            }],
            usage_metadata: None,
            model_version: None,
        };

        let result = convert_response(&google_response, "claude-sonnet-4-5");

        assert_eq!(result.content.len(), 2);
        assert!(result.content[0].is_text());
        assert!(result.content[1].is_tool_use());
    }

    #[test]
    fn test_generate_message_id() {
        let id1 = generate_message_id();
        let id2 = generate_message_id();

        assert!(id1.starts_with("msg_"));
        assert!(id2.starts_with("msg_"));
        // Should be unique
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_usage_no_cache() {
        let google_response = GoogleResponse {
            candidates: vec![Candidate {
                content: Some(Content {
                    role: Some("model".to_string()),
                    parts: vec![Part::text("Test")],
                }),
                finish_reason: Some("STOP".to_string()),
                safety_ratings: vec![],
                citation_metadata: None,
                index: None,
            }],
            usage_metadata: Some(UsageMetadata {
                prompt_token_count: 100,
                candidates_token_count: 50,
                cached_content_token_count: None, // No cache
                total_token_count: 150,
                thoughts_token_count: None,
            }),
            model_version: None,
        };

        let result = convert_response(&google_response, "claude-sonnet-4-5");

        assert_eq!(result.usage.input_tokens, 100);
        assert!(result.usage.cache_read_input_tokens.is_none());
    }

    #[test]
    fn test_usage_zero_cache() {
        let google_response = GoogleResponse {
            candidates: vec![Candidate {
                content: Some(Content {
                    role: Some("model".to_string()),
                    parts: vec![Part::text("Test")],
                }),
                finish_reason: Some("STOP".to_string()),
                safety_ratings: vec![],
                citation_metadata: None,
                index: None,
            }],
            usage_metadata: Some(UsageMetadata {
                prompt_token_count: 100,
                candidates_token_count: 50,
                cached_content_token_count: Some(0), // Zero cache
                total_token_count: 150,
                thoughts_token_count: None,
            }),
            model_version: None,
        };

        let result = convert_response(&google_response, "claude-sonnet-4-5");

        assert_eq!(result.usage.input_tokens, 100);
        // Zero cache should result in None
        assert!(result.usage.cache_read_input_tokens.is_none());
    }
}
