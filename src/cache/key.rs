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
/// temperature, max_tokens, tool definitions, and tool_choice.
pub fn exact_hash(request: &ChatRequest) -> String {
    let mut hasher = Sha256::new();

    // Version prefix to allow for future hashing logic updates
    hasher.update(b"v1:");

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

    // Tool definitions
    if let Some(ref tools) = request.tools {
        match serde_json::to_string(tools) {
            Ok(tools_json) => hasher.update(tools_json.as_bytes()),
            Err(_) => hasher.update(b"<tools-serialization-error>"),
        }
    }
    hasher.update(b"|");

    // tool_choice (canonical JSON string if present)
    if let Some(ref tc) = request.tool_choice {
        hasher.update(tc.to_string().as_bytes());
    }

    format!("{:x}", hasher.finalize())
}

// ---------------------------------------------------------------------------
// Component Hashes
// ---------------------------------------------------------------------------

/// Hash the system prompt(s) specifically for metadata-aware ANN filtering.
pub fn system_prompt_hash(request: &ChatRequest) -> String {
    let mut hasher = Sha256::new();
    for msg in &request.messages {
        if matches!(msg.role, MessageRole::System) {
            if let Some(ref content) = msg.content {
                hasher.update(flatten_content(content).trim().as_bytes());
                hasher.update(b";");
            }
        }
    }
    format!("{:x}", hasher.finalize())
}

/// Hash the tool definitions for metadata-aware ANN filtering.
pub fn tool_definitions_hash(request: &ChatRequest) -> String {
    let mut hasher = Sha256::new();
    if let Some(ref tools) = request.tools {
        match serde_json::to_string(tools) {
            Ok(tools_json) => hasher.update(tools_json.as_bytes()),
            Err(_) => hasher.update(b"<tools-serialization-error>"),
        }
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
pub fn flatten_content(content: &MessageContent) -> String {
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