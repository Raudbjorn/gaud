//! AWS SSE binary event stream parser.
//!
//! Kiro uses a custom AWS event stream format (NOT standard text-based SSE).
//! Events are JSON objects within the stream, identified by patterns like
//! `{"content":`, `{"name":`, `{"input":`, `{"stop":`, `{"usage":`, etc.

use tracing::trace;

use crate::models::kiro::KiroStreamEvent;

/// Patterns that identify different event types in the stream.
const CONTENT_PATTERN: &str = r#"{"content":"#;
const TOOL_NAME_PATTERN: &str = r#"{"name":"#;
const TOOL_INPUT_PATTERN: &str = r#"{"input":"#;
const STOP_PATTERN: &str = r#"{"stop":"#;
const USAGE_PATTERN: &str = r#"{"usage":"#;
const CONTEXT_USAGE_PATTERN: &str = r#"{"contextUsagePercentage":"#;

/// Parse a raw chunk from the Kiro SSE stream into events.
///
/// A single chunk may contain multiple events (newline-separated JSON objects).
pub fn parse_chunk(chunk: &str) -> Vec<KiroStreamEvent> {
    let mut events = Vec::new();

    for line in chunk.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if let Some(event) = parse_event_line(line) {
            events.push(event);
        }
    }

    events
}

/// Parse a single event line from the stream.
fn parse_event_line(line: &str) -> Option<KiroStreamEvent> {
    // Content event: {"content":"some text"}
    if line.starts_with(CONTENT_PATTERN) {
        return parse_content_event(line);
    }

    // Tool name (start): {"name":"tool_name","toolUseId":"..."}
    if line.starts_with(TOOL_NAME_PATTERN) {
        return parse_tool_start_event(line);
    }

    // Tool input: {"input":"partial json"}
    if line.starts_with(TOOL_INPUT_PATTERN) {
        return parse_tool_input_event(line);
    }

    // Stop event: {"stop":"end_turn"}
    if line.starts_with(STOP_PATTERN) {
        return Some(KiroStreamEvent::ToolStop);
    }

    // Usage event: {"usage":{...}}
    if line.starts_with(USAGE_PATTERN) {
        return parse_usage_event(line);
    }

    // Context usage: {"contextUsagePercentage":0.42}
    if line.starts_with(CONTEXT_USAGE_PATTERN) {
        return parse_context_usage_event(line);
    }

    // Try bracket-style tool calls: [{"name":"tool","input":{...},"toolUseId":"..."}]
    if line.starts_with('[') {
        return parse_bracket_tool_calls(line);
    }

    trace!("Unrecognized stream line: {}", &line[..line.len().min(100)]);
    None
}

fn parse_content_event(line: &str) -> Option<KiroStreamEvent> {
    let data: serde_json::Value = serde_json::from_str(line).ok()?;
    let content = data.get("content")?.as_str()?;
    Some(KiroStreamEvent::Content(content.to_string()))
}

fn parse_tool_start_event(line: &str) -> Option<KiroStreamEvent> {
    let data: serde_json::Value = serde_json::from_str(line).ok()?;
    let name = data.get("name")?.as_str()?.to_string();
    let tool_use_id = data
        .get("toolUseId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let input = data
        .get("input")
        .map(|v| v.to_string())
        .unwrap_or_default();

    Some(KiroStreamEvent::ToolStart {
        name,
        tool_use_id,
        input,
    })
}

fn parse_tool_input_event(line: &str) -> Option<KiroStreamEvent> {
    let data: serde_json::Value = serde_json::from_str(line).ok()?;
    let input = data.get("input")?.as_str()?.to_string();
    Some(KiroStreamEvent::ToolInput(input))
}

fn parse_usage_event(line: &str) -> Option<KiroStreamEvent> {
    let data: serde_json::Value = serde_json::from_str(line).ok()?;
    let usage = data.get("usage")?.clone();
    Some(KiroStreamEvent::Usage(usage))
}

fn parse_context_usage_event(line: &str) -> Option<KiroStreamEvent> {
    let data: serde_json::Value = serde_json::from_str(line).ok()?;
    let pct = data.get("contextUsagePercentage")?.as_f64()?;
    Some(KiroStreamEvent::ContextUsage(pct))
}

/// Parse bracket-style tool calls: `[{"name":"...","input":{...},"toolUseId":"..."}]`
///
/// This format appears when the model returns tool calls as a JSON array
/// in the content stream rather than as separate events.
fn parse_bracket_tool_calls(line: &str) -> Option<KiroStreamEvent> {
    let data: serde_json::Value = serde_json::from_str(line).ok()?;
    let arr = data.as_array()?;

    if let Some(first) = arr.first() {
        let name = first.get("name")?.as_str()?.to_string();
        let tool_use_id = first
            .get("toolUseId")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let input = first
            .get("input")
            .map(|v| v.to_string())
            .unwrap_or_default();

        return Some(KiroStreamEvent::ToolStart {
            name,
            tool_use_id,
            input,
        });
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_content() {
        let events = parse_chunk(r#"{"content":"Hello"}"#);
        assert_eq!(events.len(), 1);
        match &events[0] {
            KiroStreamEvent::Content(text) => assert_eq!(text, "Hello"),
            _ => panic!("Expected Content event"),
        }
    }

    #[test]
    fn test_parse_tool_start() {
        let events = parse_chunk(r#"{"name":"get_weather","toolUseId":"tool_123","input":{}}"#);
        assert_eq!(events.len(), 1);
        match &events[0] {
            KiroStreamEvent::ToolStart { name, tool_use_id, .. } => {
                assert_eq!(name, "get_weather");
                assert_eq!(tool_use_id, "tool_123");
            }
            _ => panic!("Expected ToolStart event"),
        }
    }

    #[test]
    fn test_parse_usage() {
        let events = parse_chunk(r#"{"usage":{"inputTokenCount":100,"outputTokenCount":50}}"#);
        assert_eq!(events.len(), 1);
        match &events[0] {
            KiroStreamEvent::Usage(data) => {
                assert_eq!(data["inputTokenCount"], 100);
            }
            _ => panic!("Expected Usage event"),
        }
    }

    #[test]
    fn test_parse_context_usage() {
        let events = parse_chunk(r#"{"contextUsagePercentage":0.42}"#);
        assert_eq!(events.len(), 1);
        match &events[0] {
            KiroStreamEvent::ContextUsage(pct) => {
                assert!((pct - 0.42).abs() < f64::EPSILON);
            }
            _ => panic!("Expected ContextUsage event"),
        }
    }

    #[test]
    fn test_parse_multiple_lines() {
        let chunk = r#"{"content":"Hello "}
{"content":"world"}
{"usage":{"inputTokenCount":10,"outputTokenCount":2}}"#;
        let events = parse_chunk(chunk);
        assert_eq!(events.len(), 3);
    }

    #[test]
    fn test_parse_empty_lines_skipped() {
        let chunk = "\n\n{\"content\":\"Hi\"}\n\n";
        let events = parse_chunk(chunk);
        assert_eq!(events.len(), 1);
    }
}
