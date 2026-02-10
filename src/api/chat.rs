use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Instant;

use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::Extension;
use axum::Json;
use tokio_stream::Stream;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::budget::AuditEntry;
use crate::error::AppError;
use crate::providers::cost::CostCalculator;
use crate::providers::types::{ChatChunk, ChatRequest, Usage, UsageTokenDetails};
use crate::AppState;

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
            let cost = state.cost_calculator.calculate_cost(&model, &response.usage);

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

    let sse_stream = AuditingStream::new(
        chunk_stream,
        user.user_id,
        model,
        request_id,
        start,
        state.audit_tx.clone(),
        Arc::clone(&state.cost_calculator),
    );

    Ok(Sse::new(sse_stream)
        .keep_alive(KeepAlive::default())
        .into_response())
}

// ---------------------------------------------------------------------------
// AuditingStream
// ---------------------------------------------------------------------------

/// Wraps a `ChatChunk` stream, converting to SSE events while accumulating
/// token usage. Emits an `AuditEntry` with computed cost when the stream ends.
/// Wraps a `ChatChunk` stream, converting to SSE events while accumulating
/// token usage. Emits an `AuditEntry` with computed cost when the stream ends.
struct AuditingStream {
    /// The underlying provider chunk stream.
    inner: Pin<Box<dyn futures::Stream<Item = Result<ChatChunk, crate::providers::ProviderError>> + Send>>,
    /// Whether the inner stream has finished (we still need to emit [DONE]).
    inner_done: bool,
    /// Whether the [DONE] sentinel has been sent.
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
}

impl AuditingStream {
    fn new(
        chunk_stream: Pin<Box<dyn futures::Stream<Item = Result<ChatChunk, crate::providers::ProviderError>> + Send>>,
        user_id: String,
        model: String,
        request_id: String,
        start: Instant,
        audit_tx: tokio::sync::mpsc::UnboundedSender<AuditEntry>,
        cost_calculator: Arc<CostCalculator>,
    ) -> Self {
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
}

impl Stream for AuditingStream {
    type Item = Result<Event, std::convert::Infallible>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        // Phase 1: drain the inner chunk stream.
        if !this.inner_done {
            match this.inner.as_mut().poll_next(cx) {
                Poll::Ready(Some(Ok(chunk))) => {
                    this.accumulate_usage(&chunk);
                    let json = serde_json::to_string(&chunk).unwrap_or_default();
                    return Poll::Ready(Some(Ok(Event::default().data(json))));
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
                    return Poll::Ready(Some(Ok(Event::default().data(error_json.to_string()))));
                }
                Poll::Ready(None) => {
                    this.inner_done = true;
                    // Fall through to emit [DONE].
                }
                Poll::Pending => return Poll::Pending,
            }
        }

        // Phase 2: send the [DONE] sentinel.
        if !this.done_sent {
            this.done_sent = true;
            this.emit_audit();
            return Poll::Ready(Some(Ok(Event::default().data("[DONE]"))));
        }

        // Phase 3: stream is finished.
        Poll::Ready(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::types::{ChunkChoice, Delta};

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
        let chunk = ChatChunk {
            id: "chatcmpl-test".to_string(),
            object: "chat.completion.chunk".to_string(),
            created: 1700000000,
            model: "test-model".to_string(),
            choices: vec![ChunkChoice {
                index: 0,
                delta: Delta {
                    role: None,
                    content: Some("Hello".to_string()),
                    reasoning_content: None,
                    tool_calls: None,
                },
                finish_reason: None,
            }],
            usage: None,
        };

        let json = serde_json::to_string(&chunk).unwrap();
        assert!(json.contains("chat.completion.chunk"));
        assert!(json.contains("Hello"));
    }

    #[test]
    fn test_chat_chunk_with_usage() {
        let chunk = ChatChunk {
            id: "chatcmpl-test".to_string(),
            object: "chat.completion.chunk".to_string(),
            created: 1700000000,
            model: "test-model".to_string(),
            choices: vec![],
            usage: Some(Usage {
                prompt_tokens: 100,
                completion_tokens: 50,
                total_tokens: 150,
                prompt_tokens_details: None,
                completion_tokens_details: None,
            }),
        };

        let json = serde_json::to_value(&chunk).unwrap();
        assert_eq!(json["usage"]["prompt_tokens"], 100);
        assert_eq!(json["usage"]["completion_tokens"], 50);
    }

    #[test]
    fn test_done_event_format() {
        let done = "[DONE]";
        assert_eq!(done, "[DONE]");
    }

    #[test]
    fn test_accumulate_usage() {
        let cost_calc = Arc::new(CostCalculator::new());
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let stream = futures::stream::empty();
        let mut auditing = AuditingStream::new(
            Box::pin(stream),
            "user1".into(),
            "claude-sonnet-4-20250514".into(),
            "req1".into(),
            Instant::now(),
            tx,
            cost_calc,
        );

        // First chunk has partial usage.
        let chunk1 = ChatChunk {
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
        auditing.accumulate_usage(&chunk1);
        assert_eq!(auditing.input_tokens, 100);
        assert_eq!(auditing.output_tokens, 10);

        // Second chunk with final usage (higher values win).
        let chunk2 = ChatChunk {
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
        auditing.accumulate_usage(&chunk2);
        assert_eq!(auditing.input_tokens, 100);
        assert_eq!(auditing.output_tokens, 50);

        // Chunk without usage doesn't change accumulation.
        let chunk3 = ChatChunk {
            id: "c3".into(),
            object: "chat.completion.chunk".into(),
            created: 0,
            model: "test".into(),
            choices: vec![],
            usage: None,
        };
        auditing.accumulate_usage(&chunk3);
        assert_eq!(auditing.input_tokens, 100);
        assert_eq!(auditing.output_tokens, 50);
    }

    #[test]
    fn test_accumulate_cached_tokens() {
        let cost_calc = Arc::new(CostCalculator::new());
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let stream = futures::stream::empty();
        let mut auditing = AuditingStream::new(
            Box::pin(stream),
            "user1".into(),
            "claude-sonnet-4-20250514".into(),
            "req1".into(),
            Instant::now(),
            tx,
            cost_calc,
        );

        // Chunk with cached tokens
        let chunk = ChatChunk {
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
        auditing.accumulate_usage(&chunk);
        assert_eq!(auditing.input_tokens, 1000);
        assert_eq!(auditing.output_tokens, 50);
        assert_eq!(auditing.cached_tokens, Some(800));

        // Second chunk with higher cached token count
        let chunk2 = ChatChunk {
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
        auditing.accumulate_usage(&chunk2);
        assert_eq!(auditing.cached_tokens, Some(900));
    }
}
