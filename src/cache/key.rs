use sha2::{Digest, Sha256};

use crate::config::CacheConfig;
use crate::providers::types::{ChatRequest, ContentPart, MessageContent, MessageRole};

// ---------------------------------------------------------------------------
// Skip conditions
// ---------------------------------------------------------------------------

/// Returns `true` if this request should bypass the cache entirely.
pub fn should_skip(request: &ChatRequest, config: &CacheConfig) -> bool {
    // Streaming requests are not cached (Phase 1).
    if request.stream {
        return true;
    }

    // Tool-calling requests when configured to skip.
    if config.skip_tool_requests && request.tools.is_some() {
        return true;
    }

    // Model in skip list.
    if config.skip_models.iter().any(|m| m == &request.model) {
        return true;
    }

    false
}

// ---------------------------------------------------------------------------
// Exact-match hash (SHA-256 of canonical request fields)
// ---------------------------------------------------------------------------

/// Compute a deterministic SHA-256 hex digest from the cache-relevant fields
/// of a `ChatRequest`. The hash is model-scoped and includes message content,
/// temperature, max_tokens, tool presence, and tool_choice.
pub fn exact_hash(request: &ChatRequest) -> String {
    let mut hasher = Sha256::new();

    // Model
    hasher.update(request.model.as_bytes());
    hasher.update(b"|");

    // Messages: normalize all content to plain text, trim whitespace
    for msg in &request.messages {
        hasher.update(format!("{:?}", msg.role).as_bytes());
        hasher.update(b":");
        if let Some(ref content) = msg.content {
            let text = flatten_content(content);
            hasher.update(text.trim().as_bytes());
        }
        hasher.update(b";");
    }
    hasher.update(b"|");

    // Temperature rounded to 2 decimal places
    if let Some(temp) = request.temperature {
        let rounded = (temp * 100.0).round() / 100.0;
        hasher.update(format!("{rounded:.2}").as_bytes());
    }
    hasher.update(b"|");

    // max_tokens
    if let Some(max) = request.max_tokens {
        hasher.update(max.to_string().as_bytes());
    }
    hasher.update(b"|");

    // Tools presence (bool), not full tool definitions
    hasher.update(if request.tools.is_some() { b"T" } else { b"F" });
    hasher.update(b"|");

    // tool_choice (canonical JSON string if present)
    if let Some(ref tc) = request.tool_choice {
        hasher.update(tc.to_string().as_bytes());
    }

    format!("{:x}", hasher.finalize())
}

// ---------------------------------------------------------------------------
// Semantic text extraction
// ---------------------------------------------------------------------------

/// Extract the text to be embedded for semantic similarity search.
///
/// Format: system message (if any) + "\n---\n" + last user message content.
/// Truncated to 8192 chars.
const MAX_SEMANTIC_LEN: usize = 8192;

pub fn semantic_text(request: &ChatRequest) -> String {
    let mut parts: Vec<String> = Vec::new();

    // System message(s)
    for msg in &request.messages {
        if matches!(msg.role, MessageRole::System) {
            if let Some(ref content) = msg.content {
                let text = flatten_content(content);
                if !text.is_empty() {
                    parts.push(text);
                }
            }
        }
    }

    // Last user message
    if let Some(msg) = request.messages.iter().rev().find(|m| matches!(m.role, MessageRole::User))
    {
        if let Some(ref content) = msg.content {
            let text = flatten_content(content);
            if !text.is_empty() {
                parts.push(text);
            }
        }
    }

    let joined = parts.join("\n---\n");
    if joined.len() > MAX_SEMANTIC_LEN {
        joined[..MAX_SEMANTIC_LEN].to_string()
    } else {
        joined
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Flatten `MessageContent` (Text or Parts) into a single plain-text string.
fn flatten_content(content: &MessageContent) -> String {
    match content {
        MessageContent::Text(s) => s.clone(),
        MessageContent::Parts(parts) => parts
            .iter()
            .filter_map(|p| match p {
                ContentPart::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" "),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::types::ChatMessage;

    fn make_msg(role: MessageRole, text: &str) -> ChatMessage {
        ChatMessage {
            role,
            content: Some(MessageContent::Text(text.to_string())),
            name: None,
            tool_calls: None,
            tool_call_id: None,
        }
    }

    fn make_request(messages: Vec<ChatMessage>) -> ChatRequest {
        ChatRequest {
            model: "gpt-4".to_string(),
            messages,
            temperature: Some(0.7),
            max_tokens: Some(100),
            stream: false,
            top_p: None,
            stop: None,
            tools: None,
            tool_choice: None,
            stream_options: None,
        }
    }

    #[test]
    fn test_exact_hash_deterministic() {
        let req = make_request(vec![make_msg(MessageRole::User, "Hello")]);
        let h1 = exact_hash(&req);
        let h2 = exact_hash(&req);
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64); // SHA-256 hex
    }

    #[test]
    fn test_exact_hash_differs_on_model() {
        let mut req1 = make_request(vec![make_msg(MessageRole::User, "Hello")]);
        let mut req2 = req1.clone();
        req1.model = "gpt-4".into();
        req2.model = "gpt-3.5-turbo".into();
        assert_ne!(exact_hash(&req1), exact_hash(&req2));
    }

    #[test]
    fn test_exact_hash_differs_on_content() {
        let req1 = make_request(vec![make_msg(MessageRole::User, "Hello")]);
        let req2 = make_request(vec![make_msg(MessageRole::User, "Goodbye")]);
        assert_ne!(exact_hash(&req1), exact_hash(&req2));
    }

    #[test]
    fn test_exact_hash_whitespace_trim() {
        let req1 = make_request(vec![make_msg(MessageRole::User, "Hello ")]);
        let req2 = make_request(vec![make_msg(MessageRole::User, "Hello")]);
        assert_eq!(exact_hash(&req1), exact_hash(&req2));
    }

    #[test]
    fn test_semantic_text_system_and_user() {
        let req = make_request(vec![
            make_msg(MessageRole::System, "You are helpful."),
            make_msg(MessageRole::User, "What is 2+2?"),
        ]);
        let text = semantic_text(&req);
        assert!(text.contains("You are helpful."));
        assert!(text.contains("---"));
        assert!(text.contains("What is 2+2?"));
    }

    #[test]
    fn test_semantic_text_user_only() {
        let req = make_request(vec![make_msg(MessageRole::User, "Hello")]);
        let text = semantic_text(&req);
        assert_eq!(text, "Hello");
    }

    #[test]
    fn test_semantic_text_last_user_message() {
        let req = make_request(vec![
            make_msg(MessageRole::User, "First"),
            make_msg(MessageRole::Assistant, "Reply"),
            make_msg(MessageRole::User, "Second"),
        ]);
        let text = semantic_text(&req);
        assert!(text.contains("Second"));
        assert!(!text.contains("First"));
    }

    #[test]
    fn test_semantic_text_truncation() {
        let long_text = "x".repeat(10000);
        let req = make_request(vec![make_msg(MessageRole::User, &long_text)]);
        let text = semantic_text(&req);
        assert_eq!(text.len(), MAX_SEMANTIC_LEN);
    }

    #[test]
    fn test_should_skip_streaming() {
        let mut req = make_request(vec![make_msg(MessageRole::User, "Hi")]);
        req.stream = true;
        let config = CacheConfig::default();
        assert!(should_skip(&req, &config));
    }

    #[test]
    fn test_should_skip_tools() {
        let mut req = make_request(vec![make_msg(MessageRole::User, "Hi")]);
        req.tools = Some(vec![]);
        let config = CacheConfig {
            skip_tool_requests: true,
            ..Default::default()
        };
        assert!(should_skip(&req, &config));
    }

    #[test]
    fn test_should_skip_model() {
        let req = make_request(vec![make_msg(MessageRole::User, "Hi")]);
        let config = CacheConfig {
            skip_models: vec!["gpt-4".to_string()],
            ..Default::default()
        };
        assert!(should_skip(&req, &config));
    }

    #[test]
    fn test_should_not_skip_normal_request() {
        let req = make_request(vec![make_msg(MessageRole::User, "Hi")]);
        let config = CacheConfig::default();
        assert!(!should_skip(&req, &config));
    }

    #[test]
    fn test_flatten_content_text() {
        let content = MessageContent::Text("hello world".into());
        assert_eq!(flatten_content(&content), "hello world");
    }

    #[test]
    fn test_flatten_content_parts() {
        let content = MessageContent::Parts(vec![
            ContentPart::Text {
                text: "hello".into(),
            },
            ContentPart::Text {
                text: "world".into(),
            },
        ]);
        assert_eq!(flatten_content(&content), "hello world");
    }
}
