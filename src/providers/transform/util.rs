//! Shared conversion utilities for provider transformations.
//!
//! Contains functions migrated from `transformer.rs` plus new utilities
//! for system message handling, message alternation, tool ID sanitization,
//! and Gemini-specific conversions.

use crate::providers::types::*;

// MARK: - System Messages

/// Concatenate all system messages into a single string, joined by `\n\n`.
pub fn concatenate_system_messages(messages: &[ChatMessage]) -> Option<String> {
    let system_texts: Vec<&str> = messages
        .iter()
        .filter(|m| matches!(m.role, MessageRole::System))
        .filter_map(|m| m.content.as_ref())
        .map(|c| c.as_text())
        .filter(|t| !t.is_empty())
        .collect();

    if system_texts.is_empty() {
        None
    } else {
        Some(system_texts.join("\n\n"))
    }
}

/// Extract the first system message from messages array.
pub fn extract_system_message(messages: &[ChatMessage]) -> Option<String> {
    messages
        .iter()
        .find(|m| matches!(m.role, MessageRole::System))
        .and_then(|m| m.content.as_ref())
        .map(|c| c.as_text().to_string())
}

/// Filter out system messages from messages array.
pub fn filter_system_messages(messages: &[ChatMessage]) -> Vec<ChatMessage> {
    messages
        .iter()
        .filter(|m| !matches!(m.role, MessageRole::System))
        .cloned()
        .collect()
}

// MARK: - Message Ordering

/// Enforce user/assistant alternation by merging consecutive same-role messages.
///
/// Required by the Anthropic API which rejects non-alternating messages.
/// Consecutive messages with the same role are merged by concatenating their
/// text content with `\n\n`.
pub fn enforce_message_alternation(messages: Vec<ChatMessage>) -> Vec<ChatMessage> {
    let mut result: Vec<ChatMessage> = Vec::with_capacity(messages.len());

    for msg in messages {
        if let Some(last) = result.last_mut() {
            let same_role = matches!(
                (&last.role, &msg.role),
                (MessageRole::User, MessageRole::User)
                    | (MessageRole::Assistant, MessageRole::Assistant)
            );

            if same_role {
                // Merge: concatenate text content
                if let (Some(existing), Some(new)) = (&last.content, &msg.content) {
                    let merged_text =
                        format!("{}\n\n{}", existing.as_text(), new.as_text());
                    last.content = Some(MessageContent::Text(merged_text));
                }

                // Merge tool_calls from assistant messages
                if matches!(last.role, MessageRole::Assistant) {
                    if let Some(new_calls) = msg.tool_calls {
                        let calls = last.tool_calls.get_or_insert_with(Vec::new);
                        calls.extend(new_calls);
                    }
                }

                continue;
            }
        }
        result.push(msg);
    }

    result
}

// MARK: - Tool Conversion (Anthropic)

/// Convert OpenAI tool_choice to Anthropic format.
pub fn convert_tool_choice(
    tool_choice: &Option<serde_json::Value>,
    parallel_tool_calls: Option<bool>,
) -> Option<serde_json::Value> {
    let choice = tool_choice.as_ref()?;

    // Handle string values: "auto", "required", "none"
    if let Some(s) = choice.as_str() {
        return match s {
            "auto" => Some(serde_json::json!({"type": "auto"})),
            "required" => Some(serde_json::json!({"type": "any"})),
            "none" => Some(serde_json::json!({"type": "none"})),
            _ => None,
        };
    }

    // Handle object values with function specification
    if let Some(obj) = choice.as_object() {
        if let Some(func) = obj.get("function") {
            if let Some(name) = func.get("name").and_then(|n| n.as_str()) {
                let mut result = serde_json::json!({
                    "type": "tool",
                    "name": name
                });

                if let Some(parallel) = parallel_tool_calls {
                    result["disable_parallel_tool_use"] = serde_json::Value::Bool(!parallel);
                }

                return Some(result);
            }
        }

        // Handle direct type specification
        if let Some(type_str) = obj.get("type").and_then(|t| t.as_str()) {
            return match type_str {
                "auto" => Some(serde_json::json!({"type": "auto"})),
                "required" | "any" => Some(serde_json::json!({"type": "any"})),
                "none" => Some(serde_json::json!({"type": "none"})),
                _ => None,
            };
        }
    }

    None
}

/// Convert OpenAI tools to Anthropic tool format.
pub fn convert_tools_to_anthropic(tools: &[Tool]) -> Vec<serde_json::Value> {
    tools
        .iter()
        .map(|tool| {
            let mut anthropic_tool = serde_json::json!({
                "name": tool.function.name,
                "input_schema": tool.function.parameters.clone().unwrap_or_else(|| {
                    serde_json::json!({
                        "type": "object",
                        "properties": {}
                    })
                })
            });

            if let Some(desc) = &tool.function.description {
                anthropic_tool["description"] = serde_json::Value::String(desc.clone());
            }

            anthropic_tool
        })
        .collect()
}

/// Convert Anthropic tool_use content to OpenAI tool_calls format.
pub fn convert_anthropic_tool_use_to_openai(
    tool_use_id: &str,
    tool_name: &str,
    tool_input: &serde_json::Value,
    index: Option<u32>,
) -> ToolCall {
    ToolCall {
        index,
        id: sanitize_tool_call_id(tool_use_id),
        r#type: "function".to_string(),
        function: FunctionCall {
            name: tool_name.to_string(),
            arguments: serde_json::to_string(tool_input).unwrap_or_else(|_| "{}".to_string()),
        },
    }
}

// MARK: - Tool Conversion (Gemini)

/// Convert OpenAI tools to Gemini FunctionDeclaration format.
pub fn convert_tools_to_gemini(tools: &[Tool]) -> Vec<serde_json::Value> {
    let declarations: Vec<serde_json::Value> = tools
        .iter()
        .map(|tool| {
            let mut decl = serde_json::json!({
                "name": tool.function.name,
            });

            if let Some(desc) = &tool.function.description {
                decl["description"] = serde_json::Value::String(desc.clone());
            }

            if let Some(params) = &tool.function.parameters {
                decl["parameters"] = params.clone();
            }

            decl
        })
        .collect();

    vec![serde_json::json!({
        "functionDeclarations": declarations
    })]
}

/// Convert OpenAI tool_choice to Gemini FunctionCallingConfig.
pub fn convert_tool_choice_gemini(
    tool_choice: &Option<serde_json::Value>,
) -> Option<serde_json::Value> {
    let choice = tool_choice.as_ref()?;

    if let Some(s) = choice.as_str() {
        return match s {
            "auto" => Some(serde_json::json!({"mode": "AUTO"})),
            "required" => Some(serde_json::json!({"mode": "ANY"})),
            "none" => Some(serde_json::json!({"mode": "NONE"})),
            _ => None,
        };
    }

    // Handle object with function name
    if let Some(obj) = choice.as_object() {
        if let Some(func) = obj.get("function") {
            if let Some(name) = func.get("name").and_then(|n| n.as_str()) {
                return Some(serde_json::json!({
                    "mode": "ANY",
                    "allowed_function_names": [name]
                }));
            }
        }
    }

    None
}

// MARK: - Tool ID Sanitization

/// Sanitize a tool call ID for Anthropic compatibility.
///
/// Anthropic requires tool call IDs to match `^[a-zA-Z0-9_-]+$`.
/// Replaces any non-matching characters with underscores.
pub fn sanitize_tool_call_id(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

// MARK: - Image URL Parsing

/// Parse an image URL into (source_type, media_type, data).
///
/// For `data:` URIs, returns `("base64", media_type, base64_data)`.
/// For external URLs, returns `("url", "image/png", url)`.
pub fn parse_image_url(url: &str) -> (String, String, String) {
    if url.starts_with("data:") {
        let parts: Vec<&str> = url.splitn(2, ',').collect();
        let header = parts.first().unwrap_or(&"");
        let data = parts.get(1).unwrap_or(&"");

        let media_type = header
            .trim_start_matches("data:")
            .split(';')
            .next()
            .unwrap_or("image/png");

        (
            "base64".to_string(),
            media_type.to_string(),
            data.to_string(),
        )
    } else {
        ("url".to_string(), "image/png".to_string(), url.to_string())
    }
}

// MARK: - Stop Sequences

/// Normalize stop sequences into a vector of strings.
pub fn normalize_stop_sequences(stop: &Option<StopSequence>) -> Option<Vec<String>> {
    stop.as_ref().map(|s| match s {
        StopSequence::Single(s) => vec![s.clone()],
        StopSequence::Multiple(v) => v.clone(),
    })
}

// MARK: - Finish Reason Mapping

/// Map provider-specific finish reasons to OpenAI format.
pub fn map_finish_reason_to_openai(reason: &str) -> &'static str {
    match reason {
        // Anthropic
        "end_turn" | "stop_sequence" => "stop",
        "max_tokens" => "length",
        "tool_use" => "tool_calls",
        // Gemini
        "STOP" => "stop",
        "MAX_TOKENS" => "length",
        "SAFETY" | "RECITATION" => "content_filter",
        // Already OpenAI format
        "stop" => "stop",
        "length" => "length",
        "tool_calls" => "tool_calls",
        "content_filter" => "content_filter",
        _ => "stop",
    }
}

// MARK: - Context Window Error Detection

/// Detect context window / prompt-too-long errors from provider responses.
///
/// Returns a `ProviderError::ContextWindowExceeded` if the status code and
/// response body indicate the request exceeded the model's context window.
pub fn detect_context_window_error(
    status: u16,
    body: &str,
    provider: &str,
) -> Option<crate::providers::ProviderError> {
    // Only 400-class errors can be context window errors.
    if status != 400 {
        return None;
    }

    let patterns = [
        "context_length_exceeded",
        "prompt is too long",
        "max_tokens",
        "maximum context length",
        "token limit",
        "too many tokens",
        "input is too long",
        "exceeds the maximum",
        "RESOURCE_EXHAUSTED",
    ];

    let body_lower = body.to_lowercase();
    if patterns.iter().any(|p| body_lower.contains(&p.to_lowercase())) {
        Some(crate::providers::ProviderError::ContextWindowExceeded {
            provider: provider.to_string(),
            message: body.to_string(),
            max_tokens: None,
        })
    } else {
        None
    }
}

// MARK: - Rate Limit Header Parsing

/// Parse rate limit headers from a provider's HTTP response.
///
/// Extracts and normalizes provider-specific rate limit headers into the
/// `ProviderResponseMeta` struct. Handles:
/// - Standard `Retry-After` header
/// - Anthropic's `anthropic-ratelimit-*` headers
/// - OpenAI/Copilot's `x-ratelimit-*` headers
/// - Google's `retry-after` header
pub fn parse_rate_limit_headers(
    headers: &[(String, String)],
    provider: &str,
) -> (Option<std::time::Duration>, Vec<(String, String)>) {
    let mut retry_after: Option<std::time::Duration> = None;
    let mut normalized: Vec<(String, String)> = Vec::new();

    for (name, value) in headers {
        let lower = name.to_lowercase();

        // Parse Retry-After header (standard).
        if lower == "retry-after" {
            if let Ok(secs) = value.parse::<u64>() {
                retry_after = Some(std::time::Duration::from_secs(secs));
            }
            normalized.push(("x-ratelimit-retry-after".to_string(), value.clone()));
            continue;
        }

        match provider {
            "claude" => {
                // Anthropic-specific: anthropic-ratelimit-requests-remaining, etc.
                if lower.starts_with("anthropic-ratelimit-") {
                    let suffix = lower.trim_start_matches("anthropic-ratelimit-");
                    let openai_name = format!("x-ratelimit-{suffix}");
                    normalized.push((openai_name, value.clone()));

                    // Parse reset timestamp for retry_after.
                    if suffix.ends_with("-reset") {
                        if let Ok(reset) = chrono::DateTime::parse_from_rfc3339(value) {
                            let now = chrono::Utc::now();
                            let until = reset.with_timezone(&chrono::Utc);
                            if until > now {
                                let dur = (until - now).to_std().unwrap_or_default();
                                if retry_after.is_none() {
                                    retry_after = Some(dur);
                                }
                            }
                        }
                    }
                }
            }
            "copilot" | "gemini" => {
                // OpenAI / Copilot / Gemini: forward x-ratelimit-* as-is.
                if lower.starts_with("x-ratelimit-") {
                    normalized.push((lower, value.clone()));
                }
            }
            _ => {}
        }
    }

    (retry_after, normalized)
}

// MARK: - Tests

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_concatenate_system_messages() {
        let messages = vec![
            ChatMessage {
                role: MessageRole::System,
                content: Some(MessageContent::Text("You are helpful.".to_string())),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            },
            ChatMessage {
                role: MessageRole::System,
                content: Some(MessageContent::Text("Be concise.".to_string())),
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

        let result = concatenate_system_messages(&messages);
        assert_eq!(result, Some("You are helpful.\n\nBe concise.".to_string()));
    }

    #[test]
    fn test_concatenate_system_messages_none() {
        let messages = vec![ChatMessage {
            role: MessageRole::User,
            content: Some(MessageContent::Text("Hello".to_string())),
            name: None,
            tool_calls: None,
            tool_call_id: None,
        }];

        assert_eq!(concatenate_system_messages(&messages), None);
    }

    #[test]
    fn test_enforce_message_alternation() {
        let messages = vec![
            ChatMessage {
                role: MessageRole::User,
                content: Some(MessageContent::Text("First".to_string())),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            },
            ChatMessage {
                role: MessageRole::User,
                content: Some(MessageContent::Text("Second".to_string())),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            },
            ChatMessage {
                role: MessageRole::Assistant,
                content: Some(MessageContent::Text("Reply".to_string())),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            },
        ];

        let result = enforce_message_alternation(messages);
        assert_eq!(result.len(), 2);
        assert_eq!(
            result[0].content.as_ref().unwrap().as_text(),
            "First\n\nSecond"
        );
    }

    #[test]
    fn test_sanitize_tool_call_id() {
        assert_eq!(sanitize_tool_call_id("call_abc123"), "call_abc123");
        assert_eq!(sanitize_tool_call_id("call.with.dots"), "call_with_dots");
        assert_eq!(
            sanitize_tool_call_id("id-with-dashes_and_underscores"),
            "id-with-dashes_and_underscores"
        );
        assert_eq!(sanitize_tool_call_id("has spaces!"), "has_spaces_");
    }

    #[test]
    fn test_convert_tool_choice_string() {
        let auto = Some(serde_json::json!("auto"));
        let result = convert_tool_choice(&auto, None);
        assert_eq!(result, Some(serde_json::json!({"type": "auto"})));

        let required = Some(serde_json::json!("required"));
        let result = convert_tool_choice(&required, None);
        assert_eq!(result, Some(serde_json::json!({"type": "any"})));

        let none_val = Some(serde_json::json!("none"));
        let result = convert_tool_choice(&none_val, None);
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
    fn test_convert_tools_to_gemini() {
        let tools = vec![Tool {
            r#type: "function".into(),
            function: FunctionDef {
                name: "get_time".into(),
                description: Some("Get current time".into()),
                parameters: Some(serde_json::json!({"type": "object", "properties": {}})),
            },
        }];

        let result = convert_tools_to_gemini(&tools);
        assert_eq!(result.len(), 1);
        let decls = result[0]["functionDeclarations"].as_array().unwrap();
        assert_eq!(decls.len(), 1);
        assert_eq!(decls[0]["name"], "get_time");
    }

    #[test]
    fn test_convert_tool_choice_gemini() {
        assert_eq!(
            convert_tool_choice_gemini(&Some(serde_json::json!("required"))),
            Some(serde_json::json!({"mode": "ANY"}))
        );
        assert_eq!(
            convert_tool_choice_gemini(&Some(serde_json::json!("none"))),
            Some(serde_json::json!({"mode": "NONE"}))
        );
        assert_eq!(
            convert_tool_choice_gemini(&Some(serde_json::json!("auto"))),
            Some(serde_json::json!({"mode": "AUTO"}))
        );

        let with_name = Some(serde_json::json!({
            "type": "function",
            "function": {"name": "search"}
        }));
        let result = convert_tool_choice_gemini(&with_name).unwrap();
        assert_eq!(result["mode"], "ANY");
        assert_eq!(result["allowed_function_names"][0], "search");
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

        assert_eq!(
            extract_system_message(&messages),
            Some("You are helpful.".to_string())
        );
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
    fn test_map_finish_reason_to_openai() {
        assert_eq!(map_finish_reason_to_openai("end_turn"), "stop");
        assert_eq!(map_finish_reason_to_openai("stop_sequence"), "stop");
        assert_eq!(map_finish_reason_to_openai("max_tokens"), "length");
        assert_eq!(map_finish_reason_to_openai("tool_use"), "tool_calls");
        assert_eq!(map_finish_reason_to_openai("STOP"), "stop");
        assert_eq!(map_finish_reason_to_openai("MAX_TOKENS"), "length");
        assert_eq!(map_finish_reason_to_openai("SAFETY"), "content_filter");
        assert_eq!(map_finish_reason_to_openai("RECITATION"), "content_filter");
        assert_eq!(map_finish_reason_to_openai("stop"), "stop");
        assert_eq!(map_finish_reason_to_openai("length"), "length");
    }

    // -- Context window error detection tests --

    #[test]
    fn test_detect_context_window_error_anthropic() {
        let body = r#"{"error": {"type": "invalid_request_error", "message": "prompt is too long: 210000 tokens > 200000 maximum"}}"#;
        let result = detect_context_window_error(400, body, "claude");
        assert!(result.is_some());
        if let Some(crate::providers::ProviderError::ContextWindowExceeded { provider, .. }) = result {
            assert_eq!(provider, "claude");
        }
    }

    #[test]
    fn test_detect_context_window_error_openai_pattern() {
        let body = r#"{"error": {"message": "This model's maximum context length is 128000 tokens"}}"#;
        let result = detect_context_window_error(400, body, "copilot");
        assert!(result.is_some());
    }

    #[test]
    fn test_detect_context_window_error_gemini_pattern() {
        let body = r#"{"error": {"status": "RESOURCE_EXHAUSTED", "message": "too many tokens"}}"#;
        let result = detect_context_window_error(400, body, "gemini");
        assert!(result.is_some());
    }

    #[test]
    fn test_detect_context_window_error_not_400() {
        let body = "prompt is too long";
        assert!(detect_context_window_error(429, body, "claude").is_none());
        assert!(detect_context_window_error(500, body, "claude").is_none());
    }

    #[test]
    fn test_detect_context_window_error_unrelated_400() {
        let body = r#"{"error": {"message": "invalid model name"}}"#;
        assert!(detect_context_window_error(400, body, "claude").is_none());
    }

    // -- Rate limit header parsing tests --

    #[test]
    fn test_parse_rate_limit_headers_retry_after() {
        let headers = vec![("Retry-After".to_string(), "30".to_string())];
        let (retry, normalized) = parse_rate_limit_headers(&headers, "claude");
        assert_eq!(retry, Some(std::time::Duration::from_secs(30)));
        assert_eq!(normalized.len(), 1);
        assert_eq!(normalized[0].0, "x-ratelimit-retry-after");
    }

    #[test]
    fn test_parse_rate_limit_headers_anthropic() {
        let headers = vec![
            ("anthropic-ratelimit-requests-remaining".to_string(), "10".to_string()),
            ("anthropic-ratelimit-requests-limit".to_string(), "100".to_string()),
        ];
        let (retry, normalized) = parse_rate_limit_headers(&headers, "claude");
        assert!(retry.is_none());
        assert_eq!(normalized.len(), 2);
        assert!(normalized.iter().any(|(k, _)| k == "x-ratelimit-requests-remaining"));
        assert!(normalized.iter().any(|(k, _)| k == "x-ratelimit-requests-limit"));
    }

    #[test]
    fn test_parse_rate_limit_headers_copilot() {
        let headers = vec![
            ("x-ratelimit-remaining".to_string(), "5".to_string()),
            ("x-ratelimit-limit".to_string(), "60".to_string()),
        ];
        let (retry, normalized) = parse_rate_limit_headers(&headers, "copilot");
        assert!(retry.is_none());
        assert_eq!(normalized.len(), 2);
    }

    #[test]
    fn test_parse_rate_limit_headers_empty() {
        let headers: Vec<(String, String)> = vec![];
        let (retry, normalized) = parse_rate_limit_headers(&headers, "claude");
        assert!(retry.is_none());
        assert!(normalized.is_empty());
    }
}
