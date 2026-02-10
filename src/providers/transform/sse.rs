//! Shared SSE (Server-Sent Events) byte-stream parser.
//!
//! Handles partial line buffering across TCP chunks, `data:` prefix stripping,
//! `[DONE]` sentinel detection, and infinite loop protection.
//!
//! Replaces the triplicated `stream::unfold` + manual buffer parsing in
//! claude.rs, gemini.rs, and copilot.rs.

use crate::providers::ProviderError;

// MARK: - Constants

/// Maximum number of identical consecutive chunks before triggering loop detection.
const MAX_IDENTICAL_CHUNKS: u32 = 100;

// MARK: - SSE Event

/// A parsed SSE event.
#[derive(Debug, Clone, PartialEq)]
pub enum SseEvent {
    /// A `data:` payload (JSON string, with the `data: ` prefix stripped).
    Data(String),
    /// The `[DONE]` sentinel, signaling end of stream.
    Done,
    /// A comment line (starting with `:`), or an empty event -- skip these.
    Skip,
}

// MARK: - SSE Parser

/// Stateful SSE byte-stream parser.
///
/// Accumulates partial lines across TCP chunk boundaries and yields complete
/// SSE events. Includes infinite loop detection.
pub struct SseParser {
    /// Buffer for partial lines that span TCP chunks.
    buffer: String,
    /// Last data payload seen, for loop detection.
    last_data: String,
    /// Count of consecutive identical data payloads.
    repeat_count: u32,
}

impl SseParser {
    /// Create a new SSE parser.
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
            last_data: String::new(),
            repeat_count: 0,
        }
    }

    /// Feed a chunk of bytes into the parser and return all complete SSE events.
    ///
    /// The input may contain partial lines -- they are buffered until the next
    /// chunk completes them. Returns `Err` if infinite loop is detected.
    pub fn feed(&mut self, chunk: &str) -> Result<Vec<SseEvent>, ProviderError> {
        self.buffer.push_str(chunk);
        let mut events = Vec::new();

        // Process complete lines from the buffer.
        while let Some(newline_pos) = self.buffer.find('\n') {
            let line: String = self.buffer.drain(..=newline_pos).collect();
            let line = line.trim_end_matches(|c| c == '\n' || c == '\r');

            if let Some(event) = self.parse_line(line)? {
                events.push(event);
            }
        }

        Ok(events)
    }

    /// Flush any remaining buffered data as a final event.
    ///
    /// Call this when the underlying stream ends to handle any trailing
    /// data that wasn't terminated with a newline.
    pub fn flush(&mut self) -> Result<Option<SseEvent>, ProviderError> {
        if self.buffer.is_empty() {
            return Ok(None);
        }

        let remaining = std::mem::take(&mut self.buffer);
        let line = remaining.trim();
        if line.is_empty() {
            return Ok(None);
        }

        self.parse_line(line)
    }

    /// Parse a single complete line into an SSE event.
    fn parse_line(&mut self, line: &str) -> Result<Option<SseEvent>, ProviderError> {
        // Skip empty lines (SSE event boundary markers)
        if line.is_empty() {
            return Ok(None);
        }

        // Skip comment lines
        if line.starts_with(':') {
            return Ok(Some(SseEvent::Skip));
        }

        // Skip event: lines (we only care about data: lines)
        if line.starts_with("event:") {
            return Ok(None);
        }

        // Handle data: lines
        if let Some(data) = line.strip_prefix("data:") {
            let data = data.trim_start();

            // Check for [DONE] sentinel
            if data == "[DONE]" {
                return Ok(Some(SseEvent::Done));
            }

            // Infinite loop detection
            if data == self.last_data {
                self.repeat_count += 1;
                if self.repeat_count >= MAX_IDENTICAL_CHUNKS {
                    return Err(ProviderError::Stream(
                        "Infinite loop detected: >100 identical consecutive SSE chunks"
                            .to_string(),
                    ));
                }
            } else {
                self.last_data = data.to_string();
                self.repeat_count = 1;
            }

            return Ok(Some(SseEvent::Data(data.to_string())));
        }

        // Some providers send raw JSON without data: prefix (e.g., Gemini in
        // some modes). Try to detect JSON objects.
        let trimmed = line.trim();
        if trimmed.starts_with('{') || trimmed.starts_with('[') {
            // Loop detection for raw JSON too
            if trimmed == self.last_data {
                self.repeat_count += 1;
                if self.repeat_count >= MAX_IDENTICAL_CHUNKS {
                    return Err(ProviderError::Stream(
                        "Infinite loop detected: >100 identical consecutive chunks".to_string(),
                    ));
                }
            } else {
                self.last_data = trimmed.to_string();
                self.repeat_count = 1;
            }

            return Ok(Some(SseEvent::Data(trimmed.to_string())));
        }

        // Unknown line format -- skip
        Ok(None)
    }
}

impl Default for SseParser {
    fn default() -> Self {
        Self::new()
    }
}

// MARK: - Tests

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_data_event() {
        let mut parser = SseParser::new();
        let events = parser
            .feed("data: {\"text\": \"hello\"}\n\n")
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            SseEvent::Data("{\"text\": \"hello\"}".to_string())
        );
    }

    #[test]
    fn test_done_sentinel() {
        let mut parser = SseParser::new();
        let events = parser.feed("data: [DONE]\n").unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], SseEvent::Done);
    }

    #[test]
    fn test_multiple_events_in_one_chunk() {
        let mut parser = SseParser::new();
        let events = parser
            .feed("data: {\"a\": 1}\n\ndata: {\"b\": 2}\n\n")
            .unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0], SseEvent::Data("{\"a\": 1}".to_string()));
        assert_eq!(events[1], SseEvent::Data("{\"b\": 2}".to_string()));
    }

    #[test]
    fn test_partial_line_buffering() {
        let mut parser = SseParser::new();

        // First chunk: partial line
        let events = parser.feed("data: {\"partia").unwrap();
        assert!(events.is_empty());

        // Second chunk: completes the line
        let events = parser.feed("l\": true}\n").unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            SseEvent::Data("{\"partial\": true}".to_string())
        );
    }

    #[test]
    fn test_comment_lines() {
        let mut parser = SseParser::new();
        let events = parser.feed(": this is a comment\n").unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], SseEvent::Skip);
    }

    #[test]
    fn test_event_prefix_skipped() {
        let mut parser = SseParser::new();
        let events = parser
            .feed("event: message_start\ndata: {\"type\": \"start\"}\n\n")
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            SseEvent::Data("{\"type\": \"start\"}".to_string())
        );
    }

    #[test]
    fn test_empty_lines_skipped() {
        let mut parser = SseParser::new();
        let events = parser.feed("\n\n\n").unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn test_carriage_return_handling() {
        let mut parser = SseParser::new();
        let events = parser.feed("data: {\"cr\": true}\r\n").unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            SseEvent::Data("{\"cr\": true}".to_string())
        );
    }

    #[test]
    fn test_infinite_loop_detection() {
        let mut parser = SseParser::new();

        // Feed identical chunks until loop detection triggers.
        // repeat_count starts at 1, so the 100th identical feed hits the limit.
        for i in 0..101 {
            let result = parser.feed("data: {\"same\": true}\n");
            if i < 99 {
                assert!(result.is_ok(), "should succeed at iteration {i}");
            } else {
                // At i=99, repeat_count reaches MAX_IDENTICAL_CHUNKS (100)
                assert!(result.is_err(), "should fail at iteration {i}");
                let err = result.unwrap_err();
                assert!(err.to_string().contains("Infinite loop detected"));
                return;
            }
        }
        panic!("loop detection never triggered");
    }

    #[test]
    fn test_loop_detection_resets_on_different_data() {
        let mut parser = SseParser::new();

        // Feed 90 identical chunks (under the 100 limit)
        for _ in 0..90 {
            parser.feed("data: {\"a\": 1}\n").unwrap();
        }

        // Different chunk resets counter
        parser.feed("data: {\"b\": 2}\n").unwrap();

        // Another 90 of the original -- should not trigger (counter was reset)
        for _ in 0..90 {
            parser.feed("data: {\"a\": 1}\n").unwrap();
        }
    }

    #[test]
    fn test_raw_json_detection() {
        let mut parser = SseParser::new();
        let events = parser.feed("{\"raw\": true}\n").unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], SseEvent::Data("{\"raw\": true}".to_string()));
    }

    #[test]
    fn test_flush_remaining_data() {
        let mut parser = SseParser::new();

        // Partial data without newline
        let events = parser.feed("data: {\"final\": true}").unwrap();
        assert!(events.is_empty());

        // Flush
        let event = parser.flush().unwrap();
        assert_eq!(
            event,
            Some(SseEvent::Data("{\"final\": true}".to_string()))
        );
    }

    #[test]
    fn test_flush_empty() {
        let mut parser = SseParser::new();
        assert_eq!(parser.flush().unwrap(), None);
    }

    #[test]
    fn test_tcp_fragmentation_simulation() {
        let mut parser = SseParser::new();

        // Simulate a real stream broken across TCP chunks
        let full = "data: {\"id\": \"msg_01\"}\n\ndata: {\"text\": \"Hello\"}\n\ndata: [DONE]\n";
        let chunks = vec![
            &full[0..10],   // "data: {\"id"
            &full[10..30],  // "\": \"msg_01\"}\n\nda"
            &full[30..],    // "ta: {\"text\": \"Hello\"}\n\ndata: [DONE]\n"
        ];

        let mut all_events = Vec::new();
        for chunk in chunks {
            all_events.extend(parser.feed(chunk).unwrap());
        }

        assert_eq!(all_events.len(), 3);
        assert_eq!(
            all_events[0],
            SseEvent::Data("{\"id\": \"msg_01\"}".to_string())
        );
        assert_eq!(
            all_events[1],
            SseEvent::Data("{\"text\": \"Hello\"}".to_string())
        );
        assert_eq!(all_events[2], SseEvent::Done);
    }
}
