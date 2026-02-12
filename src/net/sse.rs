//! Generic Server-Sent Events (SSE) stream parser.
//!
//! Handles the framing of SSE streams, yielding individual events.
//! Does NOT parse the data payload logic (Cloud Code vs others),
//! just provides the raw event.

use std::pin::Pin;
use std::task::{Context, Poll};
use bytes::Bytes;
use futures::stream::Stream;
use pin_project_lite::pin_project;


pin_project! {
    /// Generic SSE stream parser.
    ///
    /// Consumes a stream of Bytes and yields raw SSE events.
    pub struct SseStream<S> {
        #[pin]
        byte_stream: S,
        buffer: String,
        pending_events: std::collections::VecDeque<SseEvent>,
    }
}

/// A parsed SSE event.
#[derive(Debug, Clone, PartialEq)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
    pub id: Option<String>,
}

impl<S> SseStream<S>
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
{
    pub fn new(byte_stream: S) -> Self {
        Self {
            byte_stream,
            buffer: String::new(),
            pending_events: std::collections::VecDeque::new(),
        }
    }
}

impl<S> Stream for SseStream<S>
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
{
    type Item = Result<SseEvent, reqwest::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();

        // 1. Return pending events
        if let Some(event) = this.pending_events.pop_front() {
            return Poll::Ready(Some(Ok(event)));
        }

        // 2. Poll underlying stream
        loop {
            match this.byte_stream.as_mut().poll_next(cx) {
                Poll::Ready(Some(Ok(chunk))) => {
                    let text = String::from_utf8_lossy(&chunk);
                    this.buffer.push_str(&text);

                    while let Some(pos) = this.buffer.find("\n\n") {
                        let block: String = this.buffer.drain(..pos).collect();
                        this.buffer.drain(..2);

                        if let Some(event) = parse_sse_block(&block) {
                            this.pending_events.push_back(event);
                        }
                    }

                    if let Some(event) = this.pending_events.pop_front() {
                        return Poll::Ready(Some(Ok(event)));
                    }
                }
                Poll::Ready(Some(Err(e))) => return Poll::Ready(Some(Err(e))),
                Poll::Ready(None) => {
                    // Flush remaining buffer
                    if !this.buffer.is_empty() {
                         if let Some(event) = parse_sse_block(this.buffer) {
                             this.pending_events.push_back(event);
                         }
                         this.buffer.clear();
                    }

                    if let Some(event) = this.pending_events.pop_front() {
                        return Poll::Ready(Some(Ok(event)));
                    } else {
                        return Poll::Ready(None);
                    }
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

fn parse_sse_block(block: &str) -> Option<SseEvent> {
    let mut event = None;
    let mut data = String::new();
    let mut id = None;

    for line in block.lines() {
        if let Some(value) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(value.strip_prefix(' ').unwrap_or(value));
        } else if let Some(value) = line.strip_prefix("event:") {
            event = Some(value.strip_prefix(' ').unwrap_or(value).to_string());
        } else if let Some(value) = line.strip_prefix("id:") {
            id = Some(value.strip_prefix(' ').unwrap_or(value).to_string());
        }
    }

    if data.is_empty() && event.is_none() && id.is_none() {
        return None;
    }

    Some(SseEvent { event, data, id })
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::stream;
    use futures::StreamExt;

    #[tokio::test]
    async fn test_sse_parsing() {
        let input = "data: hello\n\ndata: world\nevent: message\n\n";
        let stream = stream::iter(vec![Ok(Bytes::from(input))]);
        let mut sse = SseStream::new(stream);

        let event1 = sse.next().await.unwrap().unwrap();
        assert_eq!(event1.data, "hello");

        let event2 = sse.next().await.unwrap().unwrap();
        assert_eq!(event2.data, "world");
        assert_eq!(event2.event.as_deref(), Some("message"));
    }
}
