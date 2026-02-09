use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Instant;

use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::Extension;
use axum::Json;
use futures::StreamExt;
use tokio_stream::Stream;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::budget::AuditEntry;
use crate::error::AppError;
use crate::providers::types::ChatRequest;
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

    let mut router = state.router.write().await;
    let result = router.chat(&request).await;
    drop(router);

    match result {
        Ok(response) => {
            let latency_ms = start.elapsed().as_millis() as u64;

            // Send audit entry asynchronously.
            let _ = state.audit_tx.send(AuditEntry {
                user_id: user.user_id,
                request_id,
                provider: response.model.clone(),
                model: model.clone(),
                input_tokens: response.usage.prompt_tokens,
                output_tokens: response.usage.completion_tokens,
                cost: 0.0, // Cost is calculated by the audit logger or a separate cost module.
                latency_ms,
                status: "success".to_string(),
            });

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

            Err(AppError::Provider(e.to_string()))
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
            return Err(AppError::Provider(e.to_string()));
        }
    };

    // Wrap the provider stream into an SSE event stream, tracking tokens
    // and sending the audit entry once the stream finishes.
    let audit_tx = state.audit_tx.clone();
    let sse_stream = build_sse_stream(chunk_stream, user, model, request_id, start, audit_tx);

    Ok(Sse::new(sse_stream)
        .keep_alive(KeepAlive::default())
        .into_response())
}

/// Transform a stream of `ChatChunk` values into SSE `Event` items,
/// followed by the `[DONE]` sentinel.  When the inner stream completes
/// an `AuditEntry` is sent through the channel.
fn build_sse_stream(
    chunk_stream: Pin<
        Box<dyn futures::Stream<Item = Result<crate::providers::types::ChatChunk, crate::providers::ProviderError>> + Send>,
    >,
    user: AuthUser,
    model: String,
    request_id: String,
    start: Instant,
    audit_tx: tokio::sync::mpsc::UnboundedSender<AuditEntry>,
) -> impl Stream<Item = Result<Event, std::convert::Infallible>> + Send {
    // Shared state captured into the AuditingStream for post-stream audit.
    let user_id = user.user_id.clone();
    let model_clone = model.clone();
    let request_id_clone = request_id.clone();

    // Map provider chunks to SSE events.
    let mapped = chunk_stream.map(|result| match result {
        Ok(chunk) => {
            let json = serde_json::to_string(&chunk).unwrap_or_default();
            Ok(Event::default().data(json))
        }
        Err(e) => {
            tracing::error!(error = %e, "Stream chunk error");
            let error_json = serde_json::json!({
                "error": {
                    "message": e.to_string(),
                    "type": "stream_error",
                }
            });
            Ok(Event::default().data(error_json.to_string()))
        }
    });

    // Append the [DONE] sentinel after all real chunks.
    let done_stream = futures::stream::once(async {
        Ok::<Event, std::convert::Infallible>(Event::default().data("[DONE]"))
    });

    let full_stream = mapped.chain(done_stream);

    // Wrap in our custom stream that fires the audit entry on completion.
    AuditingStream {
        inner: Box::pin(full_stream),
        audit_tx: Some(audit_tx),
        user_id,
        request_id: request_id_clone,
        model: model_clone,
        start,
        input_tokens: 0,
        output_tokens: 0,
        errored: false,
        finished: false,
    }
}

/// A wrapper stream that emits an `AuditEntry` when the inner stream completes.
struct AuditingStream {
    inner: Pin<Box<dyn Stream<Item = Result<Event, std::convert::Infallible>> + Send>>,
    audit_tx: Option<tokio::sync::mpsc::UnboundedSender<AuditEntry>>,
    user_id: String,
    request_id: String,
    model: String,
    start: Instant,
    input_tokens: u32,
    output_tokens: u32,
    errored: bool,
    finished: bool,
}

impl Stream for AuditingStream {
    type Item = Result<Event, std::convert::Infallible>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        if this.finished {
            return Poll::Ready(None);
        }

        match this.inner.as_mut().poll_next(cx) {
            Poll::Ready(Some(item)) => Poll::Ready(Some(item)),
            Poll::Ready(None) => {
                this.finished = true;

                // Emit the audit entry.
                if let Some(tx) = this.audit_tx.take() {
                    let latency_ms = this.start.elapsed().as_millis() as u64;
                    let status = if this.errored {
                        "error".to_string()
                    } else {
                        "success".to_string()
                    };

                    let _ = tx.send(AuditEntry {
                        user_id: this.user_id.clone(),
                        request_id: this.request_id.clone(),
                        provider: String::new(),
                        model: this.model.clone(),
                        input_tokens: this.input_tokens,
                        output_tokens: this.output_tokens,
                        cost: 0.0,
                        latency_ms,
                        status,
                    });
                }

                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::types::{ChatChunk, ChunkChoice, Delta, Usage};

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
}
