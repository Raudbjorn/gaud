use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Instant;

use axum::Extension;
use axum::Json;
use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use tokio_stream::Stream;
use uuid::Uuid;

use crate::AppState;
use crate::auth::AuthUser;
use crate::budget::AuditEntry;
use crate::cache::StreamCacheOps;
use crate::error::AppError;
use crate::providers::cost::CostCalculator;
use crate::providers::types::{ChatChunk, ChatRequest, Usage, UsageTokenDetails};

/// POST /v1/chat/completions
///
/// OpenAI-compatible chat completion endpoint supporting both streaming
/// (SSE) and non-streaming JSON responses.
pub async fn chat_completions(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Json(request): Json<ChatRequest>,
) -> Result<Response, AppError> {
    let request_id = Uuid::new_v4().to_string();

    tracing::info!(
        request_id = %request_id,
        user_id = %user.user_id,
        model = %request.model,
        stream = request.stream,
        "Chat completion request"
    );

    if request.stream {
        handle_streaming(state, user, request, request_id).await
    } else {
        handle_non_streaming(state, user, request, request_id).await
    }
}

/// Handle a non-streaming chat completion request.
async fn handle_non_streaming(
    state: AppState,
    user: AuthUser,
    request: ChatRequest,
    request_id: String,
) -> Result<Response, AppError> {
    let start = Instant::now();
    let model = request.model.clone();

    // -- Cache lookup --
    if let Some(ref cache) = state.cache {
        if cache.should_check(&request) {
            match cache.lookup(&request).await {
                Ok(hit) if hit.is_hit() => {
                    let kind = hit.hit_kind_str().unwrap_or("unknown");
                    let entry = hit.into_entry().unwrap();

                    match serde_json::from_str::<crate::providers::types::ChatResponse>(
                        &entry.response_json,
                    ) {
                        Ok(cached_response) => {
                            let latency_ms = start.elapsed().as_millis() as u64;
                            let _ = state.audit_tx.send(AuditEntry {
                                user_id: user.user_id,
                                request_id,
                                provider: "cache".to_string(),
                                model,
                                input_tokens: 0,
                                output_tokens: 0,
                                cost: 0.0,
                                latency_ms,
                                status: format!("cache_hit_{kind}"),
                            });
                            tracing::info!(
                                cache_hit = kind,
                                latency_ms = latency_ms,
                                "Served from cache"
                            );
                            return Ok(Json(cached_response).into_response());
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "Failed to deserialize cached response");
                        }
                    }
                }
                Ok(_) => {} // Miss, proceed normally
                Err(e) => {
                    tracing::warn!(error = %e, "Cache lookup failed");
                }
            }
        }
    }

    // -- Forward to provider --
    let mut router = state.router.write().await;
    let result = router.chat(&request).await;
    drop(router);

    match result {
        Ok(response) => {
            let latency_ms = start.elapsed().as_millis() as u64;
            let cost = state
                .cost_calculator
                .calculate_cost(&model, &response.usage);

            let _ = state.audit_tx.send(AuditEntry {
                user_id: user.user_id.clone(),
                request_id,
                provider: response.model.clone(),
                model,
                input_tokens: response.usage.prompt_tokens,
                output_tokens: response.usage.completion_tokens,
                cost,
                latency_ms,
                status: "success".to_string(),
            });

            // -- Cache store (background, non-blocking) --
            if let Some(ref cache) = state.cache {
                if cache.should_check(&request) {
                    let cache = Arc::clone(cache);
                    let req = request.clone();
                    let resp = response.clone();
                    tokio::spawn(async move {
                        if let Err(e) = cache.store(&req, &resp).await {
                            tracing::warn!(error = %e, "Failed to store in cache");
                        }
                    });
                }
            }

            Ok(Json(response).into_response())
        }
        Err(e) => {
            let latency_ms = start.elapsed().as_millis() as u64;

            let _ = state.audit_tx.send(AuditEntry {
                user_id: user.user_id,
                request_id,
                provider: String::new(),
                model,
                input_tokens: 0,
                output_tokens: 0,
                cost: 0.0,
                latency_ms,
                status: format!("error: {e}"),
            });

            Err(AppError::from(e))
        }
    }
}
/// Handle a streaming chat completion request via SSE.
async fn handle_streaming(
    state: AppState,
    user: AuthUser,
    request: ChatRequest,
    request_id: String,
) -> Result<Response, AppError> {
    let start = Instant::now();
    let model = request.model.clone();

    // -- Stream cache lookup ------------------------------------------------
    if let Some(ref cache) = state.cache {
        let cache_ops: &dyn StreamCacheOps = cache.as_ref();
        if cache_ops.check_stream(&request) {
            match cache_ops.get_cached_events(&request).await {
                Ok(Some((events, kind))) => {
                    let latency_ms = start.elapsed().as_millis() as u64;
                    let _ = state.audit_tx.send(AuditEntry {
                        user_id: user.user_id,
                        request_id,
                        provider: "cache".to_string(),
                        model,
                        input_tokens: 0,
                        output_tokens: 0,
                        cost: 0.0,
                        latency_ms,
                        status: format!("stream_cache_hit_{kind}"),
                    });
                    tracing::info!(
                        cache_hit = kind,
                        events = events.len(),
                        latency_ms = latency_ms,
                        "Serving stream from cache"
                    );
                    let replay = ReplayStream::new(events);
                    return Ok(Sse::new(SseAdapter::new(replay))
                        .keep_alive(KeepAlive::default())
                        .into_response());
                }
                Ok(None) => {} // Miss, proceed to provider
                Err(e) => {
                    tracing::warn!(error = %e, "Stream cache lookup failed");
                }
            }
        }
    }

    // -- Forward to provider ------------------------------------------------
    let mut router = state.router.write().await;
    let stream_result = router.stream_chat(&request).await;
    drop(router);

    let chunk_stream = match stream_result {
        Ok(s) => s,
        Err(e) => {
            let latency_ms = start.elapsed().as_millis() as u64;
            let _ = state.audit_tx.send(AuditEntry {
                user_id: user.user_id,
                request_id,
                provider: String::new(),
                model,
                input_tokens: 0,
                output_tokens: 0,
                cost: 0.0,
                latency_ms,
                status: format!("error: {e}"),
            });
            return Err(AppError::from(e));
        }
    };

    // Resolve cache tee parameters.
    let cache_ops: Option<Arc<dyn StreamCacheOps>> = if let Some(ref cache) = state.cache {
        let ops: &dyn StreamCacheOps = cache.as_ref();
        if ops.check_stream(&request) {
            Some(Arc::clone(cache) as Arc<dyn StreamCacheOps>)
        } else {
            None
        }
    } else {
        None
    };

    let (max_events, max_bytes) = cache_ops
        .as_ref()
        .map(|c| (c.max_stream_events(), c.max_stream_bytes()))
        .unwrap_or((0, 0));

    let sse_stream = AuditingStream::new(
        chunk_stream,
        user.user_id,
        model,
        request_id,
        start,
        state.audit_tx.clone(),
        Arc::clone(&state.cost_calculator),
        cache_ops,
        Some(request),
        max_events,
        max_bytes,
    );

    Ok(Sse::new(SseAdapter::new(sse_stream))
        .keep_alive(KeepAlive::default())
        .into_response())
}

// ---------------------------------------------------------------------------
// SseMsg — testable intermediate type
// ---------------------------------------------------------------------------

/// Intermediate SSE message, yielded by the internal streams.
///
/// Tests assert on `SseMsg` values directly; the thin [`SseAdapter`] converts
/// to [`axum::response::sse::Event`] at the edge.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SseMsg {
    /// A JSON data payload (chunk or error).
    Data(String),
    /// The `[DONE]` stream terminator.
    Done,
}

// ---------------------------------------------------------------------------
// SseAdapter — SseMsg → axum::Event
// ---------------------------------------------------------------------------

/// Thin adapter that converts a `Stream<Item = SseMsg>` into the
/// `Stream<Item = Result<Event, Infallible>>` that `Sse::new` requires.
struct SseAdapter<S> {
    inner: Pin<Box<S>>,
}

impl<S> SseAdapter<S> {
    fn new(stream: S) -> Self {
        Self {
            inner: Box::pin(stream),
        }
    }
}

impl<S: Stream<Item = SseMsg>> Stream for SseAdapter<S> {
    type Item = Result<Event, std::convert::Infallible>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        match this.inner.as_mut().poll_next(cx) {
            Poll::Ready(Some(SseMsg::Data(payload))) => {
                Poll::Ready(Some(Ok(Event::default().data(payload))))
            }
            Poll::Ready(Some(SseMsg::Done)) => {
                Poll::Ready(Some(Ok(Event::default().data("[DONE]"))))
            }
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

// ---------------------------------------------------------------------------
// ReplayStream — serves cached events as SseMsg
// ---------------------------------------------------------------------------

/// Replays cached SSE event payloads as [`SseMsg`], then emits [`SseMsg::Done`].
struct ReplayStream {
    events: std::vec::IntoIter<String>,
    done_sent: bool,
}

impl ReplayStream {
    fn new(events: Vec<String>) -> Self {
        Self {
            events: events.into_iter(),
            done_sent: false,
        }
    }
}

impl Stream for ReplayStream {
    type Item = SseMsg;

    fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<SseMsg>> {
        let this = self.get_mut();
        if let Some(payload) = this.events.next() {
            return Poll::Ready(Some(SseMsg::Data(payload)));
        }
        if !this.done_sent {
            this.done_sent = true;
            return Poll::Ready(Some(SseMsg::Done));
        }
        Poll::Ready(None)
    }
}

// ---------------------------------------------------------------------------
// AuditingStream
// ---------------------------------------------------------------------------

/// Wraps a `ChatChunk` stream, converting to [`SseMsg`] while accumulating
/// token usage. Emits an `AuditEntry` with computed cost when the stream ends.
/// Optionally tees event payloads into a bounded buffer for stream cache
/// write-behind via the [`StreamCacheOps`] trait.
struct AuditingStream {
    /// The underlying provider chunk stream.
    inner: Pin<
        Box<dyn futures::Stream<Item = Result<ChatChunk, crate::providers::ProviderError>> + Send>,
    >,
    /// Whether the inner stream has finished (we still need to emit Done).
    inner_done: bool,
    /// Whether the Done sentinel has been sent.
    done_sent: bool,

    // Audit state
    audit_tx: Option<tokio::sync::mpsc::UnboundedSender<AuditEntry>>,
    cost_calculator: Arc<CostCalculator>,
    user_id: String,
    request_id: String,
    model: String,
    start: Instant,

    // Accumulated token counts from stream chunks.
    input_tokens: u32,
    output_tokens: u32,
    cached_tokens: Option<u32>,
    errored: bool,

    // Streaming cache tee (optional, trait-based for testability)
    cache: Option<Arc<dyn StreamCacheOps>>,
    cache_request: Option<ChatRequest>,
    event_log: Vec<String>,
    event_log_bytes: usize,
    event_log_enabled: bool,
    stream_cache_max_events: usize,
    stream_cache_max_bytes: usize,
}

impl AuditingStream {
    fn new(
        chunk_stream: Pin<
            Box<
                dyn futures::Stream<Item = Result<ChatChunk, crate::providers::ProviderError>>
                    + Send,
            >,
        >,
        user_id: String,
        model: String,
        request_id: String,
        start: Instant,
        audit_tx: tokio::sync::mpsc::UnboundedSender<AuditEntry>,
        cost_calculator: Arc<CostCalculator>,
        cache: Option<Arc<dyn StreamCacheOps>>,
        cache_request: Option<ChatRequest>,
        stream_cache_max_events: usize,
        stream_cache_max_bytes: usize,
    ) -> Self {
        let event_log_enabled = cache.is_some();
        Self {
            inner: chunk_stream,
            inner_done: false,
            done_sent: false,
            audit_tx: Some(audit_tx),
            cost_calculator,
            user_id,
            request_id,
            model,
            start,
            input_tokens: 0,
            output_tokens: 0,
            cached_tokens: None,
            errored: false,
            cache,
            cache_request,
            event_log: Vec::new(),
            event_log_bytes: 0,
            event_log_enabled,
            stream_cache_max_events,
            stream_cache_max_bytes,
        }
    }

    /// Extract and accumulate token usage from a chunk.
    fn accumulate_usage(&mut self, chunk: &ChatChunk) {
        if let Some(ref usage) = chunk.usage {
            // Take the maximum of seen tokens (providers report cumulative or
            // final usage in different chunks).
            if usage.prompt_tokens > self.input_tokens {
                self.input_tokens = usage.prompt_tokens;
            }
            if usage.completion_tokens > self.output_tokens {
                self.output_tokens = usage.completion_tokens;
            }

            // Track cached tokens for proper cost calculation
            if let Some(ref details) = usage.prompt_tokens_details {
                if let Some(cached) = details.cached_tokens {
                    self.cached_tokens = Some(cached.max(self.cached_tokens.unwrap_or(0)));
                }
            }
        }
    }

    /// Buffer an event payload for stream cache, disabling if bounds exceeded.
    fn maybe_push_event(&mut self, payload: &str) {
        if !self.event_log_enabled {
            return;
        }
        let new_bytes = self.event_log_bytes + payload.len();
        if self.event_log.len() >= self.stream_cache_max_events
            || new_bytes > self.stream_cache_max_bytes
        {
            tracing::debug!(
                events = self.event_log.len(),
                bytes = new_bytes,
                "Stream cache buffer exceeded bounds, disabling caching for this request"
            );
            self.event_log_enabled = false;
            self.event_log.clear();
            self.event_log.shrink_to_fit();
            return;
        }
        self.event_log_bytes = new_bytes;
        self.event_log.push(payload.to_owned());
    }

    /// Send the audit entry with accumulated tokens and computed cost.
    fn emit_audit(&mut self) {
        if let Some(tx) = self.audit_tx.take() {
            let latency_ms = self.start.elapsed().as_millis() as u64;
            let status = if self.errored {
                "error".to_string()
            } else {
                "success".to_string()
            };

            let usage = Usage {
                prompt_tokens: self.input_tokens,
                completion_tokens: self.output_tokens,
                total_tokens: self.input_tokens + self.output_tokens,
                prompt_tokens_details: self.cached_tokens.map(|cached| UsageTokenDetails {
                    cached_tokens: Some(cached),
                    reasoning_tokens: None,
                }),
                completion_tokens_details: None,
            };
            let cost = self.cost_calculator.calculate_cost(&self.model, &usage);

            let _ = tx.send(AuditEntry {
                user_id: self.user_id.clone(),
                request_id: self.request_id.clone(),
                provider: String::new(),
                model: self.model.clone(),
                input_tokens: self.input_tokens,
                output_tokens: self.output_tokens,
                cost,
                latency_ms,
                status,
            });
        }
    }

    /// Flush buffered events to the stream cache (fire-and-forget).
    fn flush_event_log(&mut self) {
        if !self.event_log_enabled || self.errored || self.event_log.is_empty() {
            return;
        }
        if let (Some(cache), Some(request)) = (self.cache.take(), self.cache_request.take()) {
            let events = std::mem::take(&mut self.event_log);
            tokio::spawn(async move {
                if let Err(e) = cache.put_stream_events(&request, events).await {
                    tracing::warn!(error = %e, "Failed to store stream events in cache");
                }
            });
        }
    }
}

impl Stream for AuditingStream {
    type Item = SseMsg;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<SseMsg>> {
        let this = self.get_mut();

        // Phase 1: drain the inner chunk stream.
        if !this.inner_done {
            match this.inner.as_mut().poll_next(cx) {
                Poll::Ready(Some(Ok(chunk))) => {
                    this.accumulate_usage(&chunk);
                    let json = serde_json::to_string(&chunk).unwrap_or_default();
                    this.maybe_push_event(&json);
                    return Poll::Ready(Some(SseMsg::Data(json)));
                }
                Poll::Ready(Some(Err(e))) => {
                    this.errored = true;
                    tracing::error!(error = %e, "Stream chunk error");
                    let error_json = serde_json::json!({
                        "error": {
                            "message": e.to_string(),
                            "type": "stream_error",
                        }
                    });
                    return Poll::Ready(Some(SseMsg::Data(error_json.to_string())));
                }
                Poll::Ready(None) => {
                    this.inner_done = true;
                    // Fall through to emit Done.
                }
                Poll::Pending => return Poll::Pending,
            }
        }

        // Phase 2: send the Done sentinel.
        if !this.done_sent {
            this.done_sent = true;
            this.emit_audit();
            this.flush_event_log();
            return Poll::Ready(Some(SseMsg::Done));
        }

        // Phase 3: stream is finished.
        Poll::Ready(None)
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::types::CacheError;
    use crate::providers::types::{ChunkChoice, Delta};
    use futures::StreamExt;
    use std::sync::Mutex;
    use tokio::sync::oneshot;

    // -----------------------------------------------------------------------
    // Test helpers
    // -----------------------------------------------------------------------

    /// Build a minimal `ChatChunk` with the given content delta.
    fn chunk(content: &str) -> ChatChunk {
        ChatChunk {
            id: "c1".into(),
            object: "chat.completion.chunk".into(),
            created: 0,
            model: "test".into(),
            choices: vec![ChunkChoice {
                index: 0,
                delta: Delta {
                    role: None,
                    content: Some(content.to_string()),
                    reasoning_content: None,
                    tool_calls: None,
                },
                finish_reason: None,
            }],
            usage: None,
        }
    }

    /// Build a chunk that carries final usage stats.
    fn usage_chunk(prompt: u32, completion: u32) -> ChatChunk {
        ChatChunk {
            id: "c-usage".into(),
            object: "chat.completion.chunk".into(),
            created: 0,
            model: "test".into(),
            choices: vec![],
            usage: Some(Usage {
                prompt_tokens: prompt,
                completion_tokens: completion,
                total_tokens: prompt + completion,
                prompt_tokens_details: None,
                completion_tokens_details: None,
            }),
        }
    }

    /// Build an `AuditingStream` without cache (for unit tests of non-cache behaviour).
    fn auditing_no_cache(
        chunks: Vec<Result<ChatChunk, crate::providers::ProviderError>>,
    ) -> (
        AuditingStream,
        tokio::sync::mpsc::UnboundedReceiver<AuditEntry>,
    ) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let stream = futures::stream::iter(chunks);
        let s = AuditingStream::new(
            Box::pin(stream),
            "user1".into(),
            "test-model".into(),
            "req1".into(),
            Instant::now(),
            tx,
            Arc::new(CostCalculator::new()),
            None,
            None,
            0,
            0,
        );
        (s, rx)
    }

    /// Collect all `SseMsg` items from a stream.
    async fn collect_msgs<S: Stream<Item = SseMsg> + Unpin>(mut s: S) -> Vec<SseMsg> {
        let mut out = Vec::new();
        while let Some(msg) = s.next().await {
            out.push(msg);
        }
        out
    }

    // -----------------------------------------------------------------------
    // FakeCache — records calls, returns configurable results (Pattern B)
    // -----------------------------------------------------------------------

    struct FakeCache {
        should_check: bool,
        lookup_result: Mutex<Option<Vec<String>>>,
        store_calls: Mutex<Vec<Vec<String>>>,
        store_notify: Mutex<Option<oneshot::Sender<()>>>,
        max_events: usize,
        max_bytes: usize,
    }

    impl FakeCache {
        fn new() -> Self {
            Self {
                should_check: true,
                lookup_result: Mutex::new(None),
                store_calls: Mutex::new(Vec::new()),
                store_notify: Mutex::new(None),
                max_events: 2000,
                max_bytes: 8 * 1024 * 1024,
            }
        }

        fn with_hit(events: Vec<String>) -> Self {
            let f = Self::new();
            *f.lookup_result.lock().unwrap() = Some(events);
            f
        }

        fn with_limits(max_events: usize, max_bytes: usize) -> Self {
            let mut f = Self::new();
            f.max_events = max_events;
            f.max_bytes = max_bytes;
            f
        }

        /// Install a oneshot sender that fires when `put_stream_events` is called.
        /// Returns the receiver so the test can `await` it.
        fn on_store(&self) -> oneshot::Receiver<()> {
            let (tx, rx) = oneshot::channel();
            *self.store_notify.lock().unwrap() = Some(tx);
            rx
        }
    }

    impl StreamCacheOps for FakeCache {
        fn check_stream(&self, _request: &ChatRequest) -> bool {
            self.should_check
        }

        fn get_cached_events<'a>(
            &'a self,
            _request: &'a ChatRequest,
        ) -> Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<Option<(Vec<String>, &'static str)>, CacheError>,
                    > + Send
                    + 'a,
            >,
        > {
            let result = self.lookup_result.lock().unwrap().clone();
            Box::pin(async move { Ok(result.map(|events| (events, "exact"))) })
        }

        fn put_stream_events<'a>(
            &'a self,
            _request: &'a ChatRequest,
            events: Vec<String>,
        ) -> Pin<Box<dyn std::future::Future<Output = Result<(), CacheError>> + Send + 'a>>
        {
            self.store_calls.lock().unwrap().push(events);
            let notify = self.store_notify.lock().unwrap().take();
            Box::pin(async move {
                if let Some(tx) = notify {
                    let _ = tx.send(());
                }
                Ok(())
            })
        }

        fn max_stream_events(&self) -> usize {
            self.max_events
        }

        fn max_stream_bytes(&self) -> usize {
            self.max_bytes
        }
    }

    /// Build a minimal `ChatRequest` for testing.
    fn test_request() -> ChatRequest {
        ChatRequest {
            model: "test-model".into(),
            messages: vec![],
            temperature: None,
            top_p: None,
            stream: true,
            stop: None,
            max_tokens: None,
            tools: None,
            tool_choice: None,
            stream_options: None,
        }
    }

    // =======================================================================
    // Layer 1: ReplayStream unit tests
    // =======================================================================

    #[tokio::test]
    async fn test_replay_order_and_done() {
        let events = vec!["a".into(), "b".into(), "c".into()];
        let stream = ReplayStream::new(events);
        let msgs = collect_msgs(stream).await;

        assert_eq!(
            msgs,
            vec![
                SseMsg::Data("a".into()),
                SseMsg::Data("b".into()),
                SseMsg::Data("c".into()),
                SseMsg::Done,
            ]
        );
    }

    #[tokio::test]
    async fn test_replay_empty() {
        let stream = ReplayStream::new(vec![]);
        let msgs = collect_msgs(stream).await;

        assert_eq!(msgs, vec![SseMsg::Done]);
    }

    // =======================================================================
    // Layer 1: Bounded buffer unit tests
    // =======================================================================

    #[test]
    fn test_buffer_overflow_by_events() {
        let (mut s, _rx) = auditing_no_cache(vec![]);
        s.event_log_enabled = true;
        s.stream_cache_max_events = 3;
        s.stream_cache_max_bytes = 1_000_000;

        s.maybe_push_event("a");
        s.maybe_push_event("b");
        s.maybe_push_event("c");
        assert_eq!(s.event_log.len(), 3);

        // 4th event exceeds the cap → buffer disabled and cleared.
        s.maybe_push_event("overflow");
        assert!(!s.event_log_enabled);
        assert!(s.event_log.is_empty());
    }

    #[test]
    fn test_buffer_overflow_by_bytes() {
        let (mut s, _rx) = auditing_no_cache(vec![]);
        s.event_log_enabled = true;
        s.stream_cache_max_events = 1000;
        s.stream_cache_max_bytes = 10;

        s.maybe_push_event("12345"); // 5 bytes, total = 5
        assert!(s.event_log_enabled);
        assert_eq!(s.event_log.len(), 1);

        s.maybe_push_event("123456"); // 6 bytes, total would be 11 > 10
        assert!(!s.event_log_enabled);
        assert!(s.event_log.is_empty());
    }

    #[test]
    fn test_buffer_disabled_when_no_cache() {
        let (s, _rx) = auditing_no_cache(vec![]);
        // With cache=None, event_log_enabled is false from the start.
        assert!(!s.event_log_enabled);
    }

    // =======================================================================
    // Layer 2: Caching wrapper (AuditingStream + event_log)
    // =======================================================================

    /// Helper: create an AuditingStream with a FakeCache attached.
    fn auditing_with_cache(
        chunks: Vec<Result<ChatChunk, crate::providers::ProviderError>>,
        cache: Arc<FakeCache>,
    ) -> (
        AuditingStream,
        tokio::sync::mpsc::UnboundedReceiver<AuditEntry>,
    ) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let stream = futures::stream::iter(chunks);
        let max_events = cache.max_events;
        let max_bytes = cache.max_bytes;
        let s = AuditingStream::new(
            Box::pin(stream),
            "user1".into(),
            "test-model".into(),
            "req1".into(),
            Instant::now(),
            tx,
            Arc::new(CostCalculator::new()),
            Some(cache as Arc<dyn StreamCacheOps>),
            Some(test_request()),
            max_events,
            max_bytes,
        );
        (s, rx)
    }

    #[tokio::test]
    async fn test_wrapper_miss_stores_events() {
        let fake = Arc::new(FakeCache::new());
        let store_rx = fake.on_store();

        let (stream, _audit_rx) = auditing_with_cache(
            vec![Ok(chunk("Hello")), Ok(chunk(" world")), Ok(chunk("!"))],
            Arc::clone(&fake),
        );

        // Drive the stream to completion, collecting payloads.
        let msgs = collect_msgs(stream).await;

        // Should have 3 Data + 1 Done.
        assert_eq!(msgs.len(), 4);
        assert_eq!(msgs[3], SseMsg::Done);

        // Wait for the spawned store task to complete.
        tokio::time::timeout(std::time::Duration::from_secs(2), store_rx)
            .await
            .expect("store should be called within 2s")
            .expect("oneshot should not be dropped");

        // Verify store was called exactly once with the 3 chunk payloads.
        let calls = fake.store_calls.lock().unwrap();
        assert_eq!(calls.len(), 1, "store should be called exactly once");
        assert_eq!(calls[0].len(), 3, "should store exactly 3 event payloads");

        // Verify each stored payload is valid JSON containing the chunk content.
        for payload in &calls[0] {
            let _: serde_json::Value =
                serde_json::from_str(payload).expect("stored event should be valid JSON");
        }
    }

    #[tokio::test]
    async fn test_wrapper_emitted_payloads_match_stored() {
        let fake = Arc::new(FakeCache::new());
        let store_rx = fake.on_store();

        let (stream, _) = auditing_with_cache(
            vec![Ok(chunk("alpha")), Ok(chunk("beta"))],
            Arc::clone(&fake),
        );

        let msgs = collect_msgs(stream).await;
        let emitted: Vec<&str> = msgs
            .iter()
            .filter_map(|m| match m {
                SseMsg::Data(s) => Some(s.as_str()),
                _ => None,
            })
            .collect();

        tokio::time::timeout(std::time::Duration::from_secs(2), store_rx)
            .await
            .unwrap()
            .unwrap();

        let calls = fake.store_calls.lock().unwrap();
        let stored: Vec<&str> = calls[0].iter().map(|s| s.as_str()).collect();
        assert_eq!(
            emitted, stored,
            "emitted payloads must match stored payloads"
        );
    }

    #[tokio::test]
    async fn test_wrapper_error_no_store() {
        let fake = Arc::new(FakeCache::new());

        let (stream, mut audit_rx) = auditing_with_cache(
            vec![
                Ok(chunk("good")),
                Err(crate::providers::ProviderError::Stream("boom".into())),
            ],
            Arc::clone(&fake),
        );

        let msgs = collect_msgs(stream).await;

        // Should have: Data(good), Data(error_json), Done.
        assert_eq!(msgs.len(), 3);
        assert!(matches!(&msgs[2], SseMsg::Done));

        // Error JSON should be present.
        if let SseMsg::Data(ref s) = msgs[1] {
            assert!(s.contains("stream_error"), "should contain error type");
        } else {
            panic!("expected Data with error JSON");
        }

        // Give the runtime a tick — no store should fire.
        tokio::task::yield_now().await;
        let calls = fake.store_calls.lock().unwrap();
        assert!(calls.is_empty(), "store should NOT be called on error");

        // Audit should report error status.
        let audit = audit_rx.try_recv().expect("audit entry should exist");
        assert!(audit.status.starts_with("error"));
    }

    #[tokio::test]
    async fn test_wrapper_overflow_no_store() {
        let fake = Arc::new(FakeCache::with_limits(2, 1_000_000));

        let (stream, _) = auditing_with_cache(
            vec![Ok(chunk("a")), Ok(chunk("b")), Ok(chunk("c"))],
            Arc::clone(&fake),
        );

        let msgs = collect_msgs(stream).await;

        // All 3 chunks + Done should still stream through.
        assert_eq!(msgs.len(), 4);

        // Give spawned tasks a tick.
        tokio::task::yield_now().await;
        let calls = fake.store_calls.lock().unwrap();
        assert!(
            calls.is_empty(),
            "store should NOT be called when buffer overflows"
        );
    }

    #[tokio::test]
    async fn test_wrapper_disabled_no_store() {
        // No cache at all.
        let (stream, _) = auditing_no_cache(vec![Ok(chunk("a")), Ok(chunk("b"))]);

        let msgs = collect_msgs(stream).await;
        assert_eq!(msgs.len(), 3); // 2 Data + Done
        // No cache means no store call — nothing to assert beyond no panic.
    }

    // =======================================================================
    // Layer 2: Token accumulation tests
    // =======================================================================

    #[test]
    fn test_accumulate_usage() {
        let (mut s, _rx) = auditing_no_cache(vec![]);

        let c1 = ChatChunk {
            id: "c1".into(),
            object: "chat.completion.chunk".into(),
            created: 0,
            model: "test".into(),
            choices: vec![],
            usage: Some(Usage {
                prompt_tokens: 100,
                completion_tokens: 10,
                total_tokens: 110,
                prompt_tokens_details: None,
                completion_tokens_details: None,
            }),
        };
        s.accumulate_usage(&c1);
        assert_eq!(s.input_tokens, 100);
        assert_eq!(s.output_tokens, 10);

        // Higher values win.
        let c2 = ChatChunk {
            id: "c2".into(),
            object: "chat.completion.chunk".into(),
            created: 0,
            model: "test".into(),
            choices: vec![],
            usage: Some(Usage {
                prompt_tokens: 100,
                completion_tokens: 50,
                total_tokens: 150,
                prompt_tokens_details: None,
                completion_tokens_details: None,
            }),
        };
        s.accumulate_usage(&c2);
        assert_eq!(s.input_tokens, 100);
        assert_eq!(s.output_tokens, 50);

        // Chunk without usage doesn't change accumulation.
        let c3 = ChatChunk {
            id: "c3".into(),
            object: "chat.completion.chunk".into(),
            created: 0,
            model: "test".into(),
            choices: vec![],
            usage: None,
        };
        s.accumulate_usage(&c3);
        assert_eq!(s.input_tokens, 100);
        assert_eq!(s.output_tokens, 50);
    }

    #[test]
    fn test_accumulate_cached_tokens() {
        let (mut s, _rx) = auditing_no_cache(vec![]);

        let c = ChatChunk {
            id: "c1".into(),
            object: "chat.completion.chunk".into(),
            created: 0,
            model: "test".into(),
            choices: vec![],
            usage: Some(Usage {
                prompt_tokens: 1000,
                completion_tokens: 50,
                total_tokens: 1050,
                prompt_tokens_details: Some(UsageTokenDetails {
                    cached_tokens: Some(800),
                    reasoning_tokens: None,
                }),
                completion_tokens_details: None,
            }),
        };
        s.accumulate_usage(&c);
        assert_eq!(s.cached_tokens, Some(800));

        let c2 = ChatChunk {
            id: "c2".into(),
            object: "chat.completion.chunk".into(),
            created: 0,
            model: "test".into(),
            choices: vec![],
            usage: Some(Usage {
                prompt_tokens: 1000,
                completion_tokens: 100,
                total_tokens: 1100,
                prompt_tokens_details: Some(UsageTokenDetails {
                    cached_tokens: Some(900),
                    reasoning_tokens: None,
                }),
                completion_tokens_details: None,
            }),
        };
        s.accumulate_usage(&c2);
        assert_eq!(s.cached_tokens, Some(900));
    }

    // =======================================================================
    // Layer 2: Audit emission tests
    // =======================================================================

    #[tokio::test]
    async fn test_audit_emitted_on_success() {
        let (stream, mut rx) = auditing_no_cache(vec![Ok(chunk("hi")), Ok(usage_chunk(50, 10))]);
        let _msgs = collect_msgs(stream).await;

        let audit = rx.try_recv().expect("audit entry should be emitted");
        assert_eq!(audit.status, "success");
        assert_eq!(audit.input_tokens, 50);
        assert_eq!(audit.output_tokens, 10);
    }

    #[tokio::test]
    async fn test_audit_emitted_on_error() {
        let (stream, mut rx) = auditing_no_cache(vec![
            Ok(chunk("fine")),
            Err(crate::providers::ProviderError::Stream("fail".into())),
        ]);
        let _msgs = collect_msgs(stream).await;

        let audit = rx.try_recv().expect("audit entry should be emitted");
        assert_eq!(audit.status, "error");
    }

    // =======================================================================
    // Layer 1: Existing format tests (preserved)
    // =======================================================================

    #[test]
    fn test_audit_entry_creation() {
        let entry = AuditEntry {
            user_id: "user1".to_string(),
            request_id: "req1".to_string(),
            provider: "claude".to_string(),
            model: "claude-3-sonnet".to_string(),
            input_tokens: 100,
            output_tokens: 50,
            cost: 0.001,
            latency_ms: 250,
            status: "success".to_string(),
        };

        assert_eq!(entry.user_id, "user1");
        assert_eq!(entry.input_tokens, 100);
        assert_eq!(entry.output_tokens, 50);
    }

    #[test]
    fn test_chat_chunk_sse_format() {
        let c = chunk("Hello");
        let json = serde_json::to_string(&c).unwrap();
        assert!(json.contains("chat.completion.chunk"));
        assert!(json.contains("Hello"));
    }

    #[test]
    fn test_chat_chunk_with_usage() {
        let c = usage_chunk(100, 50);
        let json = serde_json::to_value(&c).unwrap();
        assert_eq!(json["usage"]["prompt_tokens"], 100);
        assert_eq!(json["usage"]["completion_tokens"], 50);
    }

    #[test]
    fn test_done_event_format() {
        let done = "[DONE]";
        assert_eq!(done, "[DONE]");
    }

    // =======================================================================
    // Layer 3: Handler-level integration (hit / miss / overflow / error)
    // =======================================================================
    // These tests exercise the same code paths as handle_streaming but
    // directly compose the streams with a FakeCache, avoiding the need
    // to construct a full AppState.

    #[tokio::test]
    async fn test_handler_cache_hit_replays() {
        // Simulate what handle_streaming does on a cache hit.
        let events = vec![r#"{"chunk":1}"#.into(), r#"{"chunk":2}"#.into()];
        let fake = Arc::new(FakeCache::with_hit(events.clone()));

        // Lookup via trait.
        let result = fake
            .get_cached_events(&test_request())
            .await
            .expect("lookup should succeed");

        assert!(result.is_some(), "should be a hit");
        let (cached_events, kind) = result.unwrap();
        assert_eq!(kind, "exact");
        assert_eq!(cached_events, events);

        // Replay stream emits the events.
        let replay = ReplayStream::new(cached_events);
        let msgs = collect_msgs(replay).await;
        assert_eq!(
            msgs,
            vec![
                SseMsg::Data(r#"{"chunk":1}"#.into()),
                SseMsg::Data(r#"{"chunk":2}"#.into()),
                SseMsg::Done,
            ]
        );
    }

    #[tokio::test]
    async fn test_handler_miss_then_store() {
        let fake = Arc::new(FakeCache::new());

        // Lookup returns None (miss).
        let result = fake
            .get_cached_events(&test_request())
            .await
            .expect("lookup should succeed");
        assert!(result.is_none());

        // Simulate provider stream → AuditingStream → collect.
        let store_rx = fake.on_store();
        let (stream, _) = auditing_with_cache(
            vec![Ok(chunk("hello")), Ok(chunk(" world"))],
            Arc::clone(&fake),
        );

        let msgs = collect_msgs(stream).await;
        assert_eq!(msgs.len(), 3); // 2 Data + Done

        // Wait for store.
        tokio::time::timeout(std::time::Duration::from_secs(2), store_rx)
            .await
            .unwrap()
            .unwrap();

        let calls = fake.store_calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].len(), 2);
    }

    #[tokio::test]
    async fn test_handler_buffer_cap_streams_fully_no_store() {
        let fake = Arc::new(FakeCache::with_limits(1, 1_000_000)); // max 1 event

        let (stream, _) =
            auditing_with_cache(vec![Ok(chunk("a")), Ok(chunk("b"))], Arc::clone(&fake));

        let msgs = collect_msgs(stream).await;
        // Both chunks + Done should arrive (streaming is not affected).
        assert_eq!(msgs.len(), 3);

        tokio::task::yield_now().await;
        let calls = fake.store_calls.lock().unwrap();
        assert!(
            calls.is_empty(),
            "should NOT store when buffer cap exceeded"
        );
    }

    #[tokio::test]
    async fn test_handler_error_no_store() {
        let fake = Arc::new(FakeCache::new());

        let (stream, mut audit_rx) = auditing_with_cache(
            vec![
                Ok(chunk("ok")),
                Err(crate::providers::ProviderError::Stream("kaboom".into())),
            ],
            Arc::clone(&fake),
        );

        let msgs = collect_msgs(stream).await;
        assert_eq!(msgs.len(), 3); // Data, Data(error), Done

        // Error event should contain the error payload.
        if let SseMsg::Data(ref s) = msgs[1] {
            assert!(s.contains("kaboom"));
        } else {
            panic!("expected error data payload");
        }

        tokio::task::yield_now().await;
        let calls = fake.store_calls.lock().unwrap();
        assert!(calls.is_empty(), "should NOT store on error");

        // Audit should have error status.
        let audit = audit_rx.try_recv().unwrap();
        assert!(audit.status.starts_with("error"));
    }

    #[tokio::test]
    async fn test_wrapper_disconnect_no_store() {
        let fake = Arc::new(FakeCache::new());
        // Use with_limits to ensure we don't hit incidental limits
        let (mut stream, _audit_rx) = auditing_with_cache(
            vec![Ok(chunk("A")), Ok(chunk("B")), Ok(chunk("C"))],
            Arc::clone(&fake),
        );

        // Read one event then drop
        let _ = stream.next().await;
        drop(stream);

        // Yield to allow background tasks to (not) run
        tokio::task::yield_now().await;
        // Small sleep to be sure no oneshot is firing
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        let calls = fake.store_calls.lock().unwrap();
        assert!(
            calls.is_empty(),
            "Dropped stream should NOT trigger storage"
        );
    }

    #[tokio::test]
    async fn test_wrapper_exact_match() {
        let fake = Arc::new(FakeCache::new());
        let store_rx = fake.on_store();

        let (stream, _audit_rx) =
            auditing_with_cache(vec![Ok(chunk("foo")), Ok(chunk("bar"))], Arc::clone(&fake));

        let msgs = collect_msgs(stream).await;
        let emitted: Vec<String> = msgs
            .into_iter()
            .filter_map(|m| match m {
                SseMsg::Data(s) => Some(s),
                _ => None,
            })
            .collect();

        // Wait for store
        tokio::time::timeout(std::time::Duration::from_secs(1), store_rx)
            .await
            .expect("storage should happen")
            .unwrap();

        let calls = fake.store_calls.lock().unwrap();
        let stored = &calls[0];

        // We expect the stored events to match the data payloads.
        // Depending on implementation, [DONE] might be included or not.
        // Our ReplayStream expects Data payloads. The AuditingStream buffers Data payloads.
        // So they should match exactly.
        assert_eq!(
            stored, &emitted,
            "Stored events must exactly match emitted data payloads"
        );
    }
}
