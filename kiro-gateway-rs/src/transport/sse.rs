//! AWS SSE binary event stream parser.
//!
//! Kiro uses a custom AWS event stream format (NOT standard text-based SSE).
//! Events are JSON objects within the stream, identified by their top-level keys
//! (e.g. `content`, `name`, `input`, `stop`, `usage`, etc.).

use tracing::{trace, warn};

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
        events.extend(parse_event_line(line));
    }

    events
}

/// Parse a single event line from the stream.
///
/// Parses the line as JSON first, then dispatches based on which keys are present
/// in the resulting object (rather than relying on prefix matching).
fn parse_event_line(line: &str) -> Vec<KiroStreamEvent> {
    // Try to parse as JSON value
    if let Ok(data) = serde_json::from_str::<serde_json::Value>(line) {
        if let Some(obj) = data.as_object() {
            if obj.contains_key("content") {
                return parse_content_event(obj).into_iter().collect();
            }
            if obj.contains_key("name") && obj.contains_key("toolUseId") {
                return parse_tool_start_event(obj).into_iter().collect();
            }
            if obj.contains_key("input") {
                return parse_tool_input_event(obj).into_iter().collect();
            }
            if obj.contains_key("stop") {
                return parse_stop_event(obj).into_iter().collect();
            }
            if obj.contains_key("usage") {
                return parse_usage_event(obj).into_iter().collect();
            }
            if obj.contains_key("contextUsagePercentage") {
                return parse_context_usage_event(obj).into_iter().collect();
            }
        }
        if data.is_array() {
            return parse_bracket_tool_calls(&data);
        }
    }

    trace!("Unrecognized stream line: {}", &line[..line.len().min(100)]);
    Vec::new()
}

fn parse_content_event(obj: &serde_json::Map<String, serde_json::Value>) -> Option<KiroStreamEvent> {
    let content = obj.get("content")?.as_str()?;
    Some(KiroStreamEvent::Content(content.to_string()))
}

fn parse_tool_start_event(obj: &serde_json::Map<String, serde_json::Value>) -> Option<KiroStreamEvent> {
    let name = obj.get("name")?.as_str()?.to_string();
    let tool_use_id = obj
        .get("toolUseId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let input = obj
        .get("input")
        .map(|v| v.to_string())
        .unwrap_or_default();

    Some(KiroStreamEvent::ToolStart {
        name,
        tool_use_id,
        input,
    })
}

fn parse_tool_input_event(obj: &serde_json::Map<String, serde_json::Value>) -> Option<KiroStreamEvent> {
    let input = obj.get("input")?.as_str()?.to_string();
    Some(KiroStreamEvent::ToolInput(input))
}

fn parse_stop_event(obj: &serde_json::Map<String, serde_json::Value>) -> Option<KiroStreamEvent> {
    let reason = obj.get("stop")?.as_str()?.to_string();
    Some(KiroStreamEvent::Stop { reason })
}

fn parse_usage_event(obj: &serde_json::Map<String, serde_json::Value>) -> Option<KiroStreamEvent> {
    let usage = obj.get("usage")?.clone();
    Some(KiroStreamEvent::Usage(usage))
}

fn parse_context_usage_event(obj: &serde_json::Map<String, serde_json::Value>) -> Option<KiroStreamEvent> {
    let pct = obj.get("contextUsagePercentage")?.as_f64()?;
    Some(KiroStreamEvent::ContextUsage(pct))
}

/// Parse bracket-style tool calls: `[{"name":"...","input":{...},"toolUseId":"..."}]`
///
/// This format appears when the model returns tool calls as a JSON array
/// in the content stream rather than as separate events.
/// Emits events for ALL elements in the array.
fn parse_bracket_tool_calls(data: &serde_json::Value) -> Vec<KiroStreamEvent> {
    let arr = match data.as_array() {
        Some(a) => a,
        None => return Vec::new(),
    };

    if arr.len() > 1 {
        warn!(
            "Bracket tool call array contains {} elements; processing all",
            arr.len()
        );
    }

    let mut events = Vec::new();
    for item in arr {
        let name = match item.get("name").and_then(|v| v.as_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        let tool_use_id = item
            .get("toolUseId")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let input = item
            .get("input")
            .map(|v| v.to_string())
            .unwrap_or_default();

        events.push(KiroStreamEvent::ToolStart {
            name,
            tool_use_id,
            input,
        });
    }

    events
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

    #[test]
    fn test_parse_stop_event() {
        let events = parse_chunk(r#"{"stop":"end_turn"}"#);
        assert_eq!(events.len(), 1);
        match &events[0] {
            KiroStreamEvent::Stop { reason } => assert_eq!(reason, "end_turn"),
            _ => panic!("Expected Stop event"),
        }
    }

    #[test]
    fn test_parse_bracket_tool_calls_multiple() {
        let events = parse_chunk(
            r#"[{"name":"tool_a","toolUseId":"id_a","input":{}},{"name":"tool_b","toolUseId":"id_b","input":{}}]"#,
        );
        assert_eq!(events.len(), 2);
        match &events[0] {
            KiroStreamEvent::ToolStart { name, tool_use_id, .. } => {
                assert_eq!(name, "tool_a");
                assert_eq!(tool_use_id, "id_a");
            }
            _ => panic!("Expected ToolStart for first element"),
        }
        match &events[1] {
            KiroStreamEvent::ToolStart { name, tool_use_id, .. } => {
                assert_eq!(name, "tool_b");
                assert_eq!(tool_use_id, "id_b");
            }
            _ => panic!("Expected ToolStart for second element"),
        }
    }

    #[test]
    fn test_parse_content_with_different_key_order() {
        // JSON key ordering should not matter with the new approach
        let events = parse_chunk(r#"{"unused":true,"content":"Hello"}"#);
        assert_eq!(events.len(), 1);
        match &events[0] {
            KiroStreamEvent::Content(text) => assert_eq!(text, "Hello"),
            _ => panic!("Expected Content event"),
        }
    }
}
