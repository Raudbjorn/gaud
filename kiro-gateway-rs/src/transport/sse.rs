//! AWS SSE binary event stream parser.
//!
//! Kiro uses a custom AWS event stream format (NOT standard text-based SSE).
//! Events are JSON objects within the stream, identified by their top-level keys
//! (e.g. `content`, `name`, `input`, `stop`, `usage`, `contextUsagePercentage`).

use tracing::trace;

use crate::models::kiro::KiroStreamEvent;

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
///
/// Parses the line as JSON first, then inspects keys to determine event type.
/// This is robust against key reordering in the JSON payload.
fn parse_event_line(line: &str) -> Option<KiroStreamEvent> {
    // Try bracket-style tool calls first: [{"name":"...","input":{...}}]
    if line.starts_with('[') {
        return parse_bracket_tool_calls(line);
    }

    let data: serde_json::Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => {
            trace!("Unparseable stream line: {}", &line[..line.len().min(100)]);
            return None;
        }
    };

    let obj = data.as_object()?;

    // Dispatch based on which keys are present (checked in priority order)
    if let Some(content) = obj.get("content").and_then(|v| v.as_str()) {
        return Some(KiroStreamEvent::Content(content.to_string()));
    }

    if obj.contains_key("name") {
        return parse_tool_start_from_value(&data);
    }

    if let Some(input) = obj.get("input").and_then(|v| v.as_str()) {
        return Some(KiroStreamEvent::ToolInput(input.to_string()));
    }

    if obj.contains_key("stop") {
        return Some(KiroStreamEvent::ToolStop);
    }

    if let Some(usage) = obj.get("usage") {
        return Some(KiroStreamEvent::Usage(usage.clone()));
    }

    if let Some(pct) = obj.get("contextUsagePercentage").and_then(|v| v.as_f64()) {
        return Some(KiroStreamEvent::ContextUsage(pct));
    }

    trace!("Unrecognized stream line: {}", &line[..line.len().min(100)]);
    None
}

fn parse_tool_start_from_value(data: &serde_json::Value) -> Option<KiroStreamEvent> {
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
    fn test_parse_content_reordered_keys() {
        // Content key is not the first key — this should still parse correctly
        let events = parse_chunk(r#"{"extra":"ignored","content":"Hello"}"#);
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
    fn test_parse_tool_start_reordered_keys() {
        let events = parse_chunk(r#"{"toolUseId":"tool_1","input":{},"name":"search"}"#);
        assert_eq!(events.len(), 1);
        match &events[0] {
            KiroStreamEvent::ToolStart { name, .. } => assert_eq!(name, "search"),
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

    #[test]
    fn test_parse_stop_event() {
        let events = parse_chunk(r#"{"stop":"end_turn"}"#);
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], KiroStreamEvent::ToolStop));
    }

    #[test]
    fn test_parse_tool_input() {
        let events = parse_chunk(r#"{"input":"partial json"}"#);
        assert_eq!(events.len(), 1);
        match &events[0] {
            KiroStreamEvent::ToolInput(input) => assert_eq!(input, "partial json"),
            _ => panic!("Expected ToolInput event"),
        }
    }

    #[test]
    fn test_parse_invalid_json_ignored() {
        let events = parse_chunk("not valid json at all");
        assert!(events.is_empty());
    }
}
